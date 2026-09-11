# M8.3f — a GPU resource cache

**Date:** 2026-09-11
**Status:** designed, approved
**Milestone:** M8.3f (Phase 8)

## 1. What this is

Every render through `GpuChain` rebuilds, from scratch, the wgpu objects it used on the previous
render: a shader module and a compute pipeline per stage, and an 82 MB staging buffer. This
milestone caches them on the process-wide `GpuContext`.

## 2. ⚠⚠ The measurement reversed the premise, and `CLAUDE.md` was wrong by ~8×

`CLAUDE.md` recorded the gap between M8.3e's measured 1.54× and its projected 1.9–2.1× as
*"~10 ms per frame is the shader module, the pipeline and every uniform and LUT buffer being
recreated on every encode"*, and flagged it as **a consistent explanation rather than a measured
one**. It was measured before this design was written, and the explanation was wrong about which
object.

`Dispatch::encode` and `GpuChain::run` were instrumented temporarily and `gpu_run_bench` run at
3200×2136 (6.8 Mpx, 82 MB) — the TYPICAL four-stage chain, warm rounds:

| item | ms |
|---|---|
| **staging buffer allocation** | **7.2** |
| GPU wait — 82 MB in, 4 dispatches, 82 MB out | 21.5 |
| readback `to_vec` of 82 MB | 6.5 |
| memcpy into the staging buffer | 2.6 |
| **encode: shader modules + pipelines + layouts + bind groups + every LUT, ×4 stages** | **1.2** |
| image buffer allocation | 0.05 |
| readback buffer allocation | 0.02 |
| submit | 0.07 |
| **total** | **~39.2** |

Two findings:

- **The pipeline and shader cache is worth 1.2 ms of ~39 — about 3%.** Per stage, the module costs
  0.10–0.12 ms and the pipeline 0.04–0.08 ms *warm*, because Metal already caches compiled shaders
  by source. Only the first render pays real compilation: round 0 shows 1.41 ms for a single
  pipeline against 0.08 warm.
- **The 7.2 ms is one buffer, and the reason is specific: `mapped_at_creation: true`.** The 82 MB
  *image* buffer costs **0.05 ms** and the 82 MB *readback* buffer **0.02 ms** — same size, same
  device, both cheap. ⚠ It is the **mapping** that is expensive, not the allocation. So
  "recreated on every encode" was the right diagnosis aimed at the wrong object.

### 2.1 The central risk was measured before designing, not after

Reusing a `MAP_WRITE` buffer means `map_async(Write)` each render instead of `mapped_at_creation`.
If that mapping cost what the allocation did, the 7.2 ms would not exist and this design would be
worthless. A standalone probe on the same buffer (⚠ it prints **78.2 MiB**, which is the 82 MB
of §2 in the other unit — `3200 × 2136 × 3 × 4` bytes either way):

| | first | steady state |
|---|---|---|
| fresh `mapped_at_creation` buffer | alloc 10.3 ms | **alloc 8.3–9.8 ms every render** |
| cached buffer + `map_async(Write)` | map 9.97 ms | **map 0.00–0.01 ms** |

Reuse replaces an 8–10 ms allocation with a 0.01 ms mapping, and the write itself gets slightly
faster on warm pages (2.4–3.5 ms against 3.8–5.5). ⚠ It is structurally safe **here** because
`GpuChain::run` already blocks on `poll(Wait)` until readback completes, so the GPU is always idle
by the time the next render maps. That property is load-bearing: a future asynchronous chain would
need a ring of buffers instead, and this spec would not survive that change unexamined.

## 3. Scope (user's call, 2026-09-11)

Both halves — the pipelines **and** the staging buffer. The user's request named the pipelines;
the measurement says they are the smaller half, and the decision was taken with both numbers in
view. ~8.4 ms of ~39.

**Not in scope:** the 8-bit readback (M8.3e's caveat 5), which is worth more than this whole
milestone and changes `GpuChain`'s contract and `pack`'s role. It stays a separate feature.

## 4. Where it lives

`crates/gpu-pipeline/src/cache.rs`, a `GpuCache` held as a field on `GpuContext`.

⚠⚠ **That placement is forced rather than chosen.** `accelerate` rebuilds `GpuChain` and every
`GpuStage` box on **every render**, so a cache owned by a chain or by a stage is discarded each
frame and buys exactly nothing. `GpuContext` is the process-wide `OnceLock` and is the only thing
in this crate that outlives a render.

Nothing in `pipeline` or `app` changes. `accelerate` is untouched. `GpuStage::encode` keeps its
signature — the cache is reached through the `ctx` it already receives.

⚠ A new file rather than growing `context.rs` (118 lines): the cache is its own concern with its
own tests, and this repo's standing rule is to decide where new code goes before writing it.

## 5. The pipeline half

```
Mutex<HashMap<String, ShaderEntry>>

ShaderEntry {
    module:    wgpu::ShaderModule,
    layout:    wgpu::BindGroupLayout,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,   // by entry point
}
```

Both `Dispatch::encode` and `GpuSharpenStage::encode` consult it. ⚠ Sharpen matters more than it
looks: it builds **one module and up to eight compute pipelines** per encode, one per entry point.

**Keyed on the full WGSL text, never on the stage's label.** Every shader today is a constant
prelude plus a constant `SHADER` — `luma_wgsl()` and sharpen's `.replace(..)` substitute
*compile-time* constants, so no shader text varies with the edits. A label key therefore works
**today** and breaks silently the first time a stage bakes a parameter into its WGSL, serving a
stale pipeline and a plausible-but-wrong picture. Hashing 2–4 KB costs ~1 µs against the 0.10 ms
module creation it removes. Fail toward a miss — `PrefixKey`'s rule.

**⚠ The layout sits under the shader key rather than under a shape key**, because sharpen's layout
differs from `Dispatch`'s and a shared shape key would collide them.

`ComputePipeline`, `ShaderModule` and `BindGroupLayout` are `#[derive(Clone)]` and internally
ref-counted in wgpu 29 — verified in the vendored source, not assumed — so the lock is held for a
hash lookup and a cheap handle clone.

## 6. The staging half

`Mutex<Option<wgpu::Buffer>>`. One entry, grown to the largest seen, never shrunk (user's call,
2026-09-11).

Each run:

- if the cached buffer is at least `bytes`: `map_async(Write)` → `poll(Wait)` → write the first
  `bytes` → `unmap`;
- otherwise: allocate exactly `bytes`, replace the cached buffer, and use it.

⚠ `copy_buffer_to_buffer` copies only `bytes`, so a larger buffer serving a smaller render is
correct **by construction** rather than by a rule anyone has to remember.

Settles at ~82 MB in practice: the TYPICAL fitted-at-rung-3200 case needs 82 MB, the fit rung
20 MB, and a zoomed native render is viewport-bounded and smaller again.

## 7. What is deliberately NOT cached, with the numbers

| object | cost per render | why not |
|---|---|---|
| image buffer | 0.05 ms | cheap — no `mapped_at_creation` |
| readback buffer | 0.02 ms | same |
| uniform buffers | ~0.01 ms/stage | contents change with the edits |
| LUT storage buffers | ~0.02 ms/stage | contents change with the edits |
| bind groups | ~0.00 ms | reference this render's image and uniform buffers |

Every one is measured. Caching them would add invalidation risk to buy nothing, and an
invalidation bug here is a **wrong picture**, not a slow one.

## 8. Testing

The cache is invisible to output, so **every existing equivalence test already covers the correct
case** — and that is exactly why they are not the guard that matters. The dangerous failure is a
cache serving a **stale** pipeline, which no equivalence test can see.

1. **Per-key miss counts.** `GpuCache` records misses per shader key. The carrier test runs one
   chain twice and asserts that key's miss count stayed at **1**.
   ⚠⚠ **Per-key rather than a total**, because the cache is genuinely process-wide and `cargo test`
   runs in parallel — a global count is the contaminated observable `GpuChain::transfer_counts`
   already records being burned by, when process-wide statics read `(6, 4)` instead of `(1, 1)`.
2. **Two different shaders render their own results.** A label-keyed cache passes a single-stage
   test and fails this one.
3. **The staging buffer is reused.** An allocation-generation counter on `GpuCache`; two runs of
   the same size must not bump it, and a larger run must.
4. **A larger cached buffer serving a smaller render is correct** — run a big chain then a small
   one and compare the small one against a cold render. This is the test that catches a
   `copy_buffer_to_buffer` given the buffer's size rather than the image's.
5. **The 63 goldens must not move**, and the gpu-pipeline equivalence tests must stay green — the
   proof that a caching change changed no pixel.

⚠ Sabotage list for the plan: key on the label instead of the text (must fail test 2); never
consult the cache (test 1); copy the whole buffer rather than `bytes` (test 4); skip the
`map_async` wait (expected to be **inert** on this backend — record it as a finding rather than
counting it as a pass).

## 9. Expected result — a prediction to be checked, not a claim

~8.4 ms of ~39 removed. `gpu_run_bench`'s TYPICAL row should go from **1.68× to roughly 2.1×**.

⚠ The real engine reads differently — `bench_gpu_tail` measured 79.58 ms CPU / 51.78 ms GPU where
this synthetic bench reads 70.05 / 41.70 — so the milestone is judged by **re-running
`bench_gpu_tail`**, not by extrapolating from the synthetic one. This project has twice been
misled by quoting a projection as a result.

## 10. Accepted caveats

1. **~82 MB held resident** once you render at rung 3200 with the tail on, never released.
2. **Nothing helps export** — `batch_export` never calls `accelerate`, and ~93% of an export is
   libraw's demosaic upstream of everything wgpu touches.
3. **`gpu_tail` is still off by default**, so a fresh install sees no change.
4. **The first render still compiles shaders** (~150 ms). `wgpu::PipelineCache` — a driver-level
   compilation cache, persistable to disk — is the answer to that and is **not** taken here: it
   addresses only the cold start, does nothing for the 0.10 ms warm module creation and nothing at
   all for the buffer, and is backend-limited.
5. **The two larger levers are untouched**: 21.5 ms of GPU wait and 6.5 ms of readback copy. ⚠ The
   **8-bit readback is worth more than this entire milestone** — it would cut the 6.5 ms `to_vec`
   to ~2.2 and shrink the device-side copy inside the GPU wait as well.
6. **The reuse safety argument depends on `run()` being synchronous.** It blocks on `poll(Wait)`
   before returning, so the GPU is idle at the next map. An asynchronous chain would need a ring
   of staging buffers, and this spec would not survive that change unexamined.
7. **Measured on one machine, one adapter (Apple M2, Metal), at one buffer size.** The ratios are
   what generalise; the absolute milliseconds are not.
