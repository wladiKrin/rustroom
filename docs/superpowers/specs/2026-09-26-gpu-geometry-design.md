# GPU geometry with a resident input

**Date:** 2026-09-26 · **Branch:** `gpu-geometry` (stacked on `drag-frame-overheads`) · **Status:** approved in brainstorm

## Why

A Straighten drag on DSCF6263 at the author's fitted rung (3200, 6.8 Mpx) costs **~63 ms a frame**
after the drag-frame-overhead fixes. The largest part, **37.4 ms**, is the CPU geometry resample.
The rest is dominated by transfers, not compute: a warm GPU prefix twin at this size spends ~33 ms in
submit+wait and ~8 ms copying 82 MB out (`probe_gpu_run_cost`, `RUSTROOM_DEBUG_XFER`). So a plain
GPU twin (upload, compute, download) would barely beat the CPU. The win needs the pixels to **stay
on the GPU** across the drag.

**Acceptance: a Straighten drag frame on DSCF6263 at rung 3200 in ~20–25 ms** (from ~63), measured
with `probe_straighten_drag`. If it lands above 35 ms, stop and report rather than tune blind.

## Design

### 1. Size-changing GPU stages
`GpuStage::output_dims(w, h) -> (usize, usize)`, defaulting to unchanged. `GpuChain` tracks the
image's logical dimensions through its stages. The buffer is allocated at the input size, and every
size-changing stage here only **shrinks or permutes** (`w'·h' <= w·h`), so the same buffer holds the
output. Deposit, quantise and readback use the **final** dimensions. Every existing stage keeps the
default.

### 2. `GpuGeometryStage`
A twin of `GeometryStage`: orientation, keystone, straighten and crop in one gather. It seeds the
three scratch planes from the image (the lens twin's pattern, since a gather cannot be in place),
then gathers into the image at the output size. The map is `FusedMap::source_of` transcribed to WGSL:
crop offset → un-rotate (skipped at angle 0) → un-orient → inverse homography (skipped when
inactive). The resampler weights are the CPU's own table, **uploaded, never transcribed** (the lens
twin's rule). ⚠ It matches the CPU to within the twins' usual tolerance, **not bit for bit**; the
CPU's inactive-keystone path uses an exact index permutation, while the GPU always maps
coordinates. It is registered in `accelerate_prefix_stage` under `"geometry"` and declines on a
degenerate frame. Region-scoped renders stay on the CPU, as for every prefix twin.

### 3. A resident input
`PrefixCache` holds **one** GPU-resident image: the checkpoint the last GPU prefix step started
from, with its `CacheKey`. On a hit, the chain copies GPU-to-GPU (`copy_buffer_to_buffer`) instead
of uploading through staging. On a miss it uploads and keeps the new image. It is not specific to
geometry: any drag of a GPU-accelerated prefix step resumes from one unchanging checkpoint.

### 4. Handoff without a download
When the **last** prefix step runs on the GPU and the tail will run entirely on the GPU (GPU tail on,
sRGB display, no proof, visualize or grade view, no region, and the tail chain covering every tail
stage), the step's output is **not downloaded**. It is deposited in `Slot::Prefix`, and the tail's
chain takes it. The tail is built **first**, from prefix output dimensions predicted through
`output_dims` (the contract `every_prefix_stages_output_dims_match_what_it_produces` already pins).

⚠ No CPU checkpoint exists for such a frame, so the final checkpoint is not cached. The worker's
**warm-behind** (the re-render on a quiet channel) extends to these frames, with handoff disabled, so
releasing the drag fills the cache exactly as a scoped render's settle does. If the tail cannot take
the plane at run time, the frame falls back to running the step on the CPU from its input
checkpoint: slower, never wrong.

### 5. Unchanged
Export and batch export use the twin with a download, like lens correction. With the GPU setting
off, nothing changes. The CPU `GeometryStage` remains the reference.

## Tests
- GPU vs CPU agreement: every orientation, angle 0/±3°, a crop, a keystone and combinations, within
  the lens twin's tolerance. The fixture must first be shown to move pixels.
- A chain with a size-changing toy stage deposits and downloads at the final size.
- A resident hit renders the same picture as a cold frame, and a drag frame performs **no upload**
  after the first (the chain's `uploads` counter).
- A handoff frame performs **no float download**, and a quiet channel afterwards fills the final
  checkpoint.
- Sabotages as usual. The drag replay's frame time is the acceptance number.

## Out of scope
- Region-scoped GPU geometry (zoomed 1:1 already renders only the visible region on the CPU).
- A GPU-resident prefix cache beyond one slot.
- Lens folded into the gather.
