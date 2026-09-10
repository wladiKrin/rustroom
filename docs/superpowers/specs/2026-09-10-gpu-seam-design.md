# M8.3a — the GPU seam and the proof

**Date:** 2026-09-10
**Status:** design approved, not yet implemented.
**Done when:** one pipeline stage runs as a `wgpu` compute shader, its output is checked against
the CPU stage by a harness, and the round-trip cost is a measurement in the repo.

⚠⚠ **This is the first of five slices, and it is deliberately the one with no user-visible
effect.** M8.3 in full is ~20 stages plus the mask machinery. The decomposition agreed with the
user (2026-09-10) is:

| | | |
|---|---|---|
| **M8.3a** | the seam, one stage, the equivalence harness | **this spec** |
| M8.3b | the resident per-pixel tail — ~11 stages, the first slice that is *faster* | ⬜ |
| M8.3c | the spatial stages — local contrast, sharpening | ⬜ |
| M8.3d | the mask loop | ⬜ |
| M8.3e | the prefix — resampling and frame-global statistics | ⬜ |

---

## 1. What was measured first

Apple M2, Metal, **headless** `wgpu` 29.0.4, release. A throwaway probe, since the riskiest
assumption in the whole milestone was that a headless device works at all.

| | |
|---|---|
| adapter | Apple M2, `IntegratedGpu`, `Metal` |
| device init | 15–17 ms, one-off |
| **upload** 19.6 MB (1600×1068 f32 RGB) | **2.4–3.0 ms** |
| **dispatch** — exposure over 1.7 M pixels | **0.04–0.09 ms** |
| **readback** | **3.2–6.9 ms** |
| **round trip** | **~6 ms** |
| GPU vs CPU exposure | **bit-exact, worst difference 0** |

Against a measured CPU tail of **12.7 ms neutral / 34.8 ms with HSL** at the fit rung
(`bench_preview`, 2026-09-10).

### ⚠⚠ The transfer is everything, and it decides the whole milestone

Compute is **0.04 ms**; moving the buffer is **~6 ms**, a factor of ~70. This confirms
`CLAUDE.md`'s standing prediction exactly — *"a single GPU stage inside a CPU pipeline is likely
net-NEGATIVE"* — and turns it from an argument into a number.

Three consequences, each of which shapes a later slice:

1. **M8.3a will make a render ~6 ms slower.** That is the price of the proof, and it is stated
   here so it is not later found and reported as a regression.
2. **M8.3b's ceiling is ~5×, not 10×.** A fully resident tail pays the transfer once and saves
   most of the compute: 34.8 ms → roughly 7 ms. The transfer is the floor.
3. ⚠⚠ **On this machine the GPU shares physical memory with the CPU, and 19.6 MB in 2.4 ms is
   ~8 GB/s — so it is *copying*, not mapping.** A zero-copy path is the single highest-value
   thing to hunt in M8.3b, ahead of porting any further stage. Do not port ten shaders and then
   discover the floor could have been removed.

### ⚠ A constraint found by hitting it

**The dispatch limit is 65535 workgroups per dimension.** A naive per-float 1D dispatch at fit
resolution needs `1600 × 1068 × 3 / 64 = 80,100` and fails validation **already**. Indexing per
*pixel* brings it to 26,700, which fits — but at the native rung (6246 × 4170 = 26 Mpx) even that
needs 407,000, so **2D dispatch is required from the start rather than retrofitted**.

This is the class of bug that passes every small fixture and fails on the first real photograph,
so §6 makes it a test rather than a memory.

## 2. Why exposure is the first stage

Because a multiply is exactly representable in both, and the gain is computed CPU-side:
`2f32.powf(ev)` becomes a uniform, so the shader contains one multiply and no `exp2`. The probe
measured **worst difference 0** — bit-exact.

That is the point. **Any divergence the harness reports for this stage is a plumbing bug, not
float drift**, which is exactly what a first slice should be able to distinguish. A
transcendental-heavy stage first would conflate "the infrastructure is wrong" with "GPU maths
differs", and this project's record is full of measurements that answered the wrong question.

⚠ **The corollary, stated rather than left implied: M8.3a does NOT answer how far a transcendental
stage diverges.** That question — whether GPU `HslStage` lands within a usable tolerance of CPU —
is M8.3b's *first* task, and it is a genuine risk to the milestone. If Oklab plus `atan2` on GPU
diverges by several 8-bit levels, M8.3b's shape changes.

## 3. The goldens keep testing the CPU (user's call)

The 63 checked-in PNGs keep testing the CPU pipeline **byte-for-byte, and never move**. A separate
harness renders the same input both ways and asserts agreement.

⚠ **The CPU path stays forever as the oracle**, the role `DecodeOptions::camera_native` already
plays for the colour matrices: *"a reference that does not move when the thing under test does is
worth far more than the few lines it costs."*

⚠ **The accepted cost, stated plainly:** once a GPU path is the default (M8.3b onward), the
goldens no longer test what ships. What protects the shipped path is the equivalence harness, so
that harness has to be as trustworthy as the goldens are — which is why §6 gives it an awkward
fixture rather than a pretty one.

Rejected: one tolerant golden set (the tolerance that accommodates GPU float differences also
hides real CPU drift, and this project has already shipped a golden blinded by clipping); and two
byte-exact sets (126 PNGs, every stage change re-authored twice, and a GPU set that Linux CI
without a GPU could never verify).

## 4. Crate layout

**A new `crates/gpu-pipeline/`**, depending on `pipeline`, `core-image`, `wgpu`, `pollster` and
`bytemuck`. All five are already in the workspace tree; nothing new is vendored.

**Nothing depends on `gpu-pipeline`.** `pipeline`, `rustroom` (the CLI), `export`, the 63 goldens
and every existing test are untouched and gain no `wgpu`.

⚠ This is what keeps the seam honest: `GpuExposureStage` implements `pipeline::PipelineStage`, so
the trait designed as the GPU seam on day one is used as exactly that, unchanged. Rejected: a
feature-gated module inside `pipeline` (feature-gated code is easy to leave broken, because CI
compiles one configuration and the developer another — the problem this repo already records for
the macOS-only menu code), and putting it in `app` (wrong side of the lib/bin split, and the CLI
could never reach it, so the seam would be proven somewhere the seam does not live).

## 5. The three components

### `GpuContext`

Holds the instance, adapter, device and queue.

- **`new() -> Option<Self>`, never a `Result`.** No adapter means the CPU path, which must always
  work. A missing GPU is not an error condition anyone should have to handle.
- **`Arc`-shared.** Creation costs 15–17 ms; one per stage would dwarf the work it enables.
- **`Send + Sync`**, which `wgpu`'s `Device` and `Queue` already are — required, because rendering
  happens on the render worker thread and not the UI thread.
- Exposes the adapter's name, type and backend, for the probe and for a future Settings readout.

### `GpuExposureStage`

Implements `pipeline::PipelineStage`. Constructed from `(ev: f32, ctx: Arc<GpuContext>)`.

`apply(ImageBuf) -> ImageBuf` uploads to a storage buffer, dispatches **2D, indexed per pixel**,
and reads back.

⚠ **The 2D dispatch is concrete rather than "somehow 2D":** with a workgroup of 64, `x` counts
groups within a row-chunk and `y` counts chunks, so the group count is
`(w·h / 64).min(65535)` by `ceil(w·h / 64 / 65535)`, and the shader recovers the linear pixel
index as `(y · 65535 + x) · 64 + local`. Both dimensions therefore stay under the limit at any
resolution this app can produce, including a 26 Mpx native rung.

⚠ **The fallback has its own seam so it is testable:** the GPU work lives in a private
`try_apply(&ImageBuf) -> Option<ImageBuf>` that returns `None` on any failure, and `apply` is
`self.try_apply(&img).unwrap_or_else(|| self.cpu.apply(img))`. Without that split the fallback
could only be exercised by unplugging a GPU, which is not a test.

⚠⚠ **`apply` returns `ImageBuf`, not `Result` — the trait has no failure channel — so every GPU
error must degrade *silently to the CPU stage*.** Buffer allocation, mapping and submission can
all fail; a render may never fail because a shader did. The fallback is not a nicety, it is what
makes the trait implementable at all.

### The shader

WGSL, one multiply per channel. The gain arrives as a uniform. No `exp2`, no branching beyond the
bounds guard.

## 6. Testing

The harness lives in `gpu-pipeline` and compares against `pipeline`'s CPU stage directly.

1. **Bit-exactness, asserted as exact** — not within a tolerance. Exposure *is* exact today
   (measured), so a tolerance here would hide the plumbing bug the test exists to catch. If this
   ever stops holding, something structural changed and it should be loud.
2. ⚠⚠ **The fixture carries negative samples and samples above 1.0.** The working space is
   scene-referred, so both exist in every real photograph — and **a shader that clamped would be
   correct on a 0..1 chart and silently destroy every highlight**. This is the same trap
   `ProfileHueSatMap`'s `v` and the display rendering's LUT index each documented.
3. ⚠⚠ **A dispatch exceeding 65535 workgroups in one dimension** — more than
   `65535 × 64 = 4,194,240` pixels, so ~2500×1700. **A 1D dispatch passes every small fixture and
   fails on the first real photograph**, which is precisely the bug already hit by hand while
   probing.
4. **Dimensions are preserved**, so a wrong stride or a truncated readback is visible.
5. **The fallback returns the CPU result**, exercised by calling `apply` on a stage whose
   `try_apply` is forced to return `None`. ⚠ The assertion is that the output equals the **CPU
   stage's** output exactly — not merely that it is non-empty, which would pass against a fallback
   that returned the input unchanged.

**Sabotage list**, each of which must fail a *named* test: clamp the shader output to `[0,1]`;
dispatch 1D; drop the bounds guard; return the input unchanged; make the fallback return the
GPU's partial result instead of the CPU's.

⚠ **What cannot be tested here, stated rather than papered over:** the `GpuContext::new() ->
None` path. This machine has a GPU, so the no-adapter branch is unreachable in these tests. It is
defence in depth, and saying so is better than inventing a fixture that appears to cover it — the
handling this repo already gave its unreachable degenerate-radius guards.

## 7. The probe

`crates/gpu-pipeline/examples/gpu_equivalence.rs` runs a real photograph's decoded buffer through
both paths and prints the divergence and the timing split — upload, dispatch, readback.

⚠ It exists so the ~6 ms cost is a **measurement in the repo** rather than a number in this
document that goes stale. `bench_thumb_cache` and `demosaic_foliage` are the precedent, and M8.2's
own spec had to be corrected because a projected figure was quoted as an achieved one.

## 8. What this deliberately does not do

1. ⚠⚠ **Nothing in the app uses it.** No Settings toggle, no substitution mechanism, nothing
   renders on the GPU when Rustroom launches. **M8.3a ships something you cannot see.** That is
   defensible only because a single GPU stage is measurably net-negative (§1) — shipping it as a
   default would be shipping a regression — and the mechanism for choosing GPU stages belongs with
   the slice that has stages worth choosing.
   ⚠ The risk this accepts is the W5 failure mode: creative LUTs shipped with four passing
   pipeline tests and were 100% non-functional because nothing carried the data to the tested
   code. The mitigation is §7's probe, which is a genuine end-to-end run — but it is a *probe*,
   not the app, and that distinction is real.
2. **It is ~6 ms slower per render**, by construction and by measurement.
3. **It does not answer transcendental divergence** — M8.3b's first task, and a genuine risk.
4. **It touches neither the prefix, the masks, nor the spatial stages.**
5. **No zero-copy and no resident buffer.** Every `apply` is a full round trip, which is the whole
   reason it is slower.
6. **Nothing here helps export**, which is ~93% demosaic inside libraw and upstream of every stage
   `wgpu` would ever port — confirmed by M8.4, and made worse by its move to `Better`.
