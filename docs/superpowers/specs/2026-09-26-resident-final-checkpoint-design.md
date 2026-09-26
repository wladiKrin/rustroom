# The resident final checkpoint: the tail's input stays on the GPU

**Date:** 2026-09-26 · **Branch:** `resident-final` (stacked on `per-pixel-fusion`) · **Status:** approved in brainstorm

## Why

After a settle, the prefix's final output lives in `PrefixCache` as a CPU image, and every
tail-slider frame clones it (2.5 ms, measured), memcpys it into a staging buffer (2.3 ms) and
copies it to the GPU before the tail runs. `docs/PIPELINE-REVIEW-2026-09-26.md` (F2) puts the
upload half at ~8–10 ms of a ~34 ms frame at rung 3200, on the most common gesture there is.

The prefix→tail seam already has a one-shot plane (`Slot::Prefix`): the frame right after a GPU
prefix step takes it and uploads nothing. But the tail *consumes* the plane, so every later
tail-slider frame uploads again; the mechanism needs a thread stamp and a clear to be safe,
because the slot is anonymous; and a handoff frame leaves no CPU checkpoint, so the warm-behind
re-runs the last prefix step and renders a whole tail nobody sees (92–108 ms on release, measured).

## The idea

One keyed GPU image: **the tail's input, kept under the key that says what it is.** `PrefixCache`
holds one `GpuImage` under `(final CacheKey, Option<Rect>)` — the cumulative prefix key, the rung,
and the region rect when the render is scoped to one. The tail's chain copies from it GPU-side
(~1 ms for 82 MB) instead of uploading. Whatever the tail's input is this frame is kept under its
key for the next: a miss uploads once. The key makes a stale hit unrepresentable, which is the
property the one-shot plane approximated with bookkeeping.

## Design

### 1. The slot

`PrefixCache` gains `resident_final: Option<(TailKey, gpu_pipeline::GpuImage)>` with
`pub(crate) type TailKey = (CacheKey, Option<pipeline::Rect>)`. One slot; any miss replaces it;
it dies with the engine, which is rebuilt per photo. It is only ever read by the tail, which
copies from it, so it is never written after it is made. Beside the existing `resident` (the
input of the step a drag resumes at), which is unchanged.

### 2. How it gets filled — three ways, one rule

The rule: whatever the tail's input is this frame, keep it on the GPU under its key.

- **A tail-slider frame** (`ran == 0`): the input is the CPU final, cropped by `rect` at 1:1. On a
  hit the tail chain runs from the resident. On a miss the chain uploads through its own cached
  staging path (the fastest upload the crate has, measured at M8.3f), copies the freshly uploaded
  image GPU-side into a new image *before its stages touch it*, runs, and hands that image back;
  the cache keeps it under the key.
- **A last prefix step on the GPU, with a download** (a settle, a cold render): the step's chain
  downloads its output for the CPU cache as today and hands its GPU output back as well. No extra
  copy: the readback reads the same buffer, and after the wait that buffer is final.
- **A handoff frame** (no download): the step's chain returns its output image, and it becomes
  the resident final. No CPU checkpoint exists, exactly as now.

A region-scoped drag of a prefix step replaces the resident every frame under `(key, rect)`,
which costs nothing beyond the upload that happens today.

### 3. The tail

`build_tail` runs as today. When the tail's CPU part is empty and the resident's key is this
frame's `(final key, rect)`, the chain runs from the resident (`Input::Resident`, bytes for the
ordinary frame, float for a proof). Otherwise the chain uploads the CPU input and keeps it (§2).
`TailFacts::prefix_offered`, `mark_shared(Slot::Prefix)` at the tail seam, `run_rgba8_from_plane`,
`run_to_plane[_from]`, the `handed`/`from_plane` branch and the handoff's fall-back-to-CPU path in
`render_cached_with` all go. `Slot::Prefix` stays for **step→step** inside the prefix loop, and
the clear at the top of that loop stays with it.

### 4. The settle

The warm-behind, when the wanted whole-frame final key has a resident and no CPU final, **downloads
the resident into the CPU cache under `Tier::Final` and stops** — about 10 ms, no tail, no frame to
discard. These are the same bytes it produces today: the last step already ran on the GPU, and
re-running it produces the same output. A settle after a region-scoped drag (no whole-frame
resident) re-renders whole-buffer, as now.

### 5. The chain's entry points

Three, replacing the plane-shaped ones, all over the one `run_with`:

- `run_rgba8_keeping_input(&ImageBuf) -> Option<(Vec<u8>, GpuImage)>` and
  `apply_keeping_input(ImageBuf) -> (ImageBuf, Option<GpuImage>)`: upload, copy the pristine input
  aside, run, download bytes or float. The tail on a miss.
- `run_rgba8_from(&GpuImage) -> Option<Vec<u8>>` beside the existing `apply_from`: the tail on a
  hit.
- `run_keeping_output(input) -> Option<(Option<ImageBuf>, GpuImage)>` with the download chosen by
  the caller: the prefix's last step, with a download on a settle and without on a handoff.
  `apply_from` stays for the intermediate prefix steps.

### 6. Observables and tests

`Ran` gains `tail_uploads: usize` (the tail chain's upload count) and `resident_final_hit: bool`.

- A second tail-slider frame uploads nothing and hits; the first uploads once.
- Changing any prefix edit between two tail frames misses, and the picture follows the edit — the
  sabotage that ignores the key fails here.
- At 1:1, two frames at one rect upload once; a pan uploads again.
- A handoff frame's resident, downloaded, equals the re-rendered final checkpoint sample for
  sample.
- A settle after a handoff fills the CPU final without running the tail: `warm_renders` and a new
  `settle_downloads` counter on the engine say which path it took.
- Every existing `render_engine` test passes unchanged in what it asserts about pixels; the ones
  that assert `handoff`, `prefix_offered` or the plane's take are rewritten to the new observables.

### 7. Measurement

`bench_gpu_tail`'s "REAL SIDECAR, global drag" row (21.6 ms today, of which the upload is 8–10)
and `probe_straighten_drag`'s `warm behind` line (92–108 ms today). Reported as measured, with the
load. Acceptance: a tail-slider frame at rung 3200 loses its upload, visible as `tail_uploads 0`
on the second frame and a `REAL SIDECAR, global drag` row near the "0 masks" row's GPU time minus
the upload.

### 8. Placement

`crates/app/src/prefix_cache.rs` (the slot and its accessor), `crates/app/src/render_engine.rs`
(the seam rewrite, which loses more lines than it gains), `crates/gpu-pipeline/src/chain.rs` (the
entry points; it is near 600 code lines and the report says so), `crates/gpu-pipeline/src/accelerate.rs`
(drop `prefix_offered`). The export path is untouched: its tail is CPU and it has no cache.

## Out of scope

- Two slots for two rungs; a rung switch costs one upload.
- The mask slot (`Slot::Mask`) and `GpuBrushRaster`'s base plane.
- The export path.
- Any change to what a checkpoint's key contains.
