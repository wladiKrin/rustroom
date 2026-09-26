# Per-pixel fusion: a run of per-pixel stages as one pass

**Date:** 2026-09-26 · **Branch:** `per-pixel-fusion` (stacked on `gpu-timestamps`) · **Status:** approved in brainstorm

## Why

Twelve of the tail's fourteen stages are pixel-in, pixel-out: white balance, exposure, contrast,
tone regions, tone curve, saturation, HSL, colour grading, vignette, display rendering, creative
look, output transform. Only local contrast and sharpening read neighbours. Today each per-pixel
stage walks the whole image and writes a fresh copy on the CPU, and is its own dispatch inside the
chain's pass on the GPU. A tail with nine of them active is nine full-frame walks on both backends
for work that is one function per pixel.

`docs/PIPELINE-REVIEW-2026-09-26.md` (F1) infers 3–4 ms per full-frame dispatch at 6.8 Mpx from
the recorded submit times, so a nine-stage per-pixel tail is ~30 ms of GPU time that one dispatch
would do in a fraction. On the CPU each stage also allocates 82 MB at rung 3200 and 313 MB at export
resolution. The GPU timestamp spec (`2026-09-26-gpu-timestamps-design.md`) measures the per-dispatch
figure before this is built; this spec's acceptance is measured with it.

## What fusion means here

For a run of consecutive per-pixel stages, walk the image **once**: for each pixel apply the first
stage's function, then the second's, and so on, and write the result. The same arithmetic on the
same values in the same order, so the CPU result is bit-identical to running the stages one after
another; no intermediate buffer ever changed a value. The saving is the reads, writes and
allocations between stages.

## Design

### 1. Grouping happens at run time, never at build time

The stage list is untouched. The builders, `PrefixKey`, the 84 uses of `stage_names()` across
the tests and examples, `accelerate`'s name matching, the region planner and the cost gate all see
exactly the list they see today. Fusion is a property of how a list is **executed**, and
`RUSTROOM_NO_FUSION=1` turns it off on both backends so one binary measures both ways.

### 2. The CPU contract: `pipeline::per_pixel`

A new module `crates/pipeline/src/per_pixel.rs`:

```rust
pub trait PerPixel: Send + Sync {
    /// The stage's function for the pixel at (x, y) of the buffer it is applied to.
    fn px(&self, x: usize, y: usize, p: [f32; 3]) -> [f32; 3];
}
```

`PipelineStage` gains a defaulted `fn per_pixel(&self) -> Option<&dyn PerPixel> { None }`. The
twelve stages implement `PerPixel` and return `Some(self)`. Position is there for the vignette,
which adds its region origin itself; the other eleven ignore it. Each of the twelve rewrites its
`apply` as one walk calling its own `px`, so `apply` and the fused walk share one definition. Where
a stage today uses `map_samples` (exposure, contrast, the composite-only tone curve, the output
transform's encode), `px` applies the same per-sample function to the three channels in order,
which is the same operations on the same values.

`per_pixel::run_stages(stages: &[Box<dyn PipelineStage>], image: ImageBuf) -> ImageBuf` walks the
list. A run of consecutive stages answering `Some` is executed as one in-place pass over
`data_mut()`: rows in parallel through rayon, pixels in order within a row, `x` counted rather than
divided, each pixel folded through the run's functions. A stage answering `None` is applied on its
own through `apply`, as today. `Pipeline::run`, `MaskStage::apply`'s sub-stage loop and
`GpuChain::cpu_fallback` call it, which is how a mask's own per-pixel sub-stages fuse (the "free
part"); the mask's copy and blend stay as they are.

A thread-local counter of fused walks, incremented on the calling thread per run, is the
observable: fused and unfused produce identical pixels, so nothing in the output can see whether
the grouping fired (the `brush::raster` precedent).

### 3. The GPU contract: `GpuStage::per_pixel`

`GpuStage` gains a defaulted `fn per_pixel(&self) -> Option<&dyn GpuPerPixel> { None }`. The twelve
twins implement `GpuPerPixel`:

```rust
pub trait GpuPerPixel {
    /// The WGSL for `fn apply_<name>(p: vec3<f32>, x: u32, y: u32, q: Params_<name>, base: u32) -> vec3<f32>`
    /// and its `struct Params_<name>`; both named after the stage.
    fn wgsl(&self) -> String;
    /// The stage's parameters, laid out as its `Params_<name>`, a multiple of 16 bytes.
    fn params(&self) -> Vec<u8>;
    /// The stage's tables, if any, in the order its function indexes them.
    fn tables(&self) -> Vec<&[f32]>;
    /// Which shared preludes the function needs (sRGB, luma, Oklab).
    fn preludes(&self) -> Preludes;
    /// Whether the function reads `x` or `y`. Only the vignette does.
    fn needs_position(&self) -> bool;
}
```

This is a restructuring of the twelve shaders, not a rewrite: the arithmetic in each `main` moves
into a function that takes its parameters by value and its table base as an argument; the
`pixels`/`row_groups` fields move into the emitter's header; the standalone `main` is deleted,
because a lone per-pixel stage is a run of length one through the same emitter. That keeps one
definition of every stage's shader. The per-stage equivalence tests, which run through
`GpuChain::apply`, then exercise the emitter with runs of one without being changed.

### 4. The emitter: `gpu_pipeline::per_pixel`

A new module `crates/gpu-pipeline/src/per_pixel.rs`. `encode_run(stages, ctx, pass, img, scratch)`
groups consecutive stages answering `Some` and emits one dispatch per group; every other stage gets
`encode` as today. `GpuChain::run_with` and `GpuMaskStage::encode` both use it, the GPU side of the
free part.

One fused dispatch binds three buffers whatever the run's length:

- **binding 0** the image, in place;
- **binding 1** one uniform buffer: a header (`pixels`, `row_groups`, `w`) followed by each
  member's `Params_<name>` at 16-byte-aligned offsets, which is what the uniform address space's
  struct alignment gives a `struct Params { hdr: Header, s0: Params_exposure, s1: Params_hsl, .. }`;
- **binding 2** one storage buffer: every member's tables concatenated, each member's base offset
  passed to its function as a literal at the call site in `main`.

The emitted text is: the union of the members' preludes, each member's struct and function, the
`Params` struct, the bindings, and a `main` that computes the pixel index, computes `x`/`y` only if
some member needs position, reads the three values, folds them through the members' functions in
order, and writes them back. The program cache keys on the text, which is the run's shape plus its
table offsets, so a drag compiles once.

`GpuChain` counts fused dispatches and plain `encode` calls separately (`encode_units()`), the
observable beside `transfer_counts()`.

### 5. What does not change

The quantise stays its own dispatch. The cost gate compares the run's summed `cpu_cost` against
`TRANSFER_COST` before any fusion happens, so its decisions are unchanged. `Dispatch` remains for
any non-per-pixel stage that uses it today. Spatial stages, masks' copy and blend, the mask slot
and the prefix seams are untouched.

### 6. Placement

`pipeline`: `per_pixel.rs` (trait, runner, counter) and a `per_pixel` impl in each of the twelve
stage files. `lib.rs` gains the `pub mod` line, the `PipelineStage` default method and the one-line
change in `Pipeline::run`; it is already 1,881 code lines and gains no more than that.
`gpu-pipeline`: `per_pixel.rs` (trait, emitter, `encode_run`), the `GpuStage` default method in
`chain.rs`, the call in `run_with` and in `mask.rs`, and each twin's restructured shader.
`render_engine` and `accelerate` are untouched.

## Order of work

1. CPU: the trait, the twelve impls, the runner, the counter; goldens must not move; the property
   test and the sabotage below.
2. GPU: the trait, the emitter with runs of one; every existing equivalence test must pass
   unchanged.
3. GPU: grouping in `encode_run`; the whole-tail test; the dispatch counter.
4. Measure, with `RUSTROOM_DEBUG_GPU_TIME` and `probe_straighten_drag`, before and after.

## Tests

- **CPU bit-identity.** All 78 goldens unmoved. A property test: on a noisy 64×48 fixture, the
  fused run of every per-pixel stage with non-neutral settings equals the sequential application
  sample for sample; the fixture check first asserts the stages move the pixels.
- **CPU order.** A new test `a_fused_run_applies_its_members_in_list_order`: exposure at +2 EV
  followed by a tone curve that clips gives a different picture from the reverse order (asserted
  first, by more than 0.05), and the fused walk of `[exposure, curve]` equals the sequential list
  order sample for sample. The existing `local_contrast_runs_after_the_tone_curve_and_before_saturation`
  pins the list itself and is untouched, because the list does not change.
- **CPU sabotage.** Folding a run in reverse order must fail the order test above and the property
  test.
- **CPU observable.** A pipeline of three per-pixel stages takes one fused walk; with
  `RUSTROOM_NO_FUSION` set on the runner (a parameter, not the process environment, in tests) it
  takes none and the pixels are identical.
- **GPU runs of one.** Every per-stage equivalence test passes unchanged through the emitter.
- **GPU whole tail.** A fixture with every per-pixel stage active, including a channel curve, a
  parametric curve, a `.cube` and a region for the vignette, renders on the GPU within the twins'
  existing bound of the CPU; first asserted to differ from the neutral render.
- **GPU observable.** A chain of nine per-pixel twins reports one fused dispatch; a sabotage that
  emits members in reverse fails the whole-tail test and the GPU twin of the order test above
  (`[exposure, curve]` on the GPU against the CPU's list order).
- **Masks.** A mask with three per-pixel sub-stages renders identically to before on both backends
  and its sub-stages take one walk (CPU) and one fused dispatch (GPU).

## Acceptance, measured

On DSCF6263 at rung 3200: the tail's GPU time from the timestamp block, before and after, and the
drag frame's median from `probe_straighten_drag`. On a photo with the full tail active: the same
two numbers. Reported as measured, with the machine's load, and with the perturbation of the timed
run stated. If the per-dispatch figure the probe reports is under 1 ms, stop after step 1 and
report rather than build the emitter for a saving that is not there.

## Out of scope

- The mask fold (no copy, no separate blend for all-per-pixel masks); its own spec, after this one
  is measured.
- Folding the quantise into the last run.
- Vectorised storage access, or any change to the image layout.
- Any change to the stage list, the builders or the keys.
