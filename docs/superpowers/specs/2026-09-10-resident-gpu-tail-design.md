# M8.3b — a resident GPU chain, and HSL across it

**Date:** 2026-09-10
**Status:** design approved, not yet implemented.

**Done when**, both halves:
1. A chain of N GPU stages costs **one** round trip rather than N, proven by **counting uploads**
   rather than by timing.
2. `HslStage` runs on the GPU behind a Settings toggle, matching the CPU stage within **0.01 of an
   8-bit level**, and the saving is measured by a probe rather than predicted here.

⚠ **The scope was widened after the first draft** (user's call, 2026-09-10). It was
"infrastructure only" — no new stage, nothing visible — and became infrastructure **plus HSL plus
the substitution mechanism plus app wiring**. The earlier reasoning that made HSL look risky was
retired by §1's measurements, which is why the wider scope is now the cheaper bet rather than the
braver one.

---

## 1. What was measured first, and what it overturned

Both questions M8.3a said to answer **before porting any stage** were answered by throwaway probes
on an Apple M2, and both changed something.

### ⚠⚠ Transcendental divergence is a non-issue — the milestone's stated risk is gone

A full Oklab round trip plus a hue rotation — `cbrt`, `atan2`, `cos`, `sin`, exactly what
`HslStage` and `ColorGradingStage` each pay per pixel — GPU against CPU:

| | |
|---|---|
| samples differing | **93.51%** |
| **worst divergence** | **0.0006 of an 8-bit level** |

Nearly every sample differs, and by **1/1600th of a level**. M8.3a recorded this as *"a genuine
risk to the milestone… if Oklab plus `atan2` on GPU diverges by several 8-bit levels, M8.3b's
shape changes."* It does not. **The equivalence tolerance for a transcendental stage can be tight.**

### ⚠⚠ Zero-copy is NOT reachable, and saying so retires M8.3a's top recommendation

M8.3a's spec said hunting a zero-copy upload was *"the single highest-value thing in M8.3b, ahead
of porting any further stage."* Measured, at fit resolution (19.6 MB):

| | |
|---|---|
| `write_buffer` — what M8.3a used | 3.90 ms |
| `mapped_at_creation` | 3.06 ms |
| **persistent `MAP_WRITE` staging buffer** | **1.86 ms** |
| *plain CPU memcpy of the same bytes* | *0.80–0.93 ms* |

There is always **one memcpy into mapped memory**; wgpu exposes no way around it, and that memcpy
is the floor. So the instinct was right and the answer is no. What remains is real but bounded:
**2.1× from the upload path alone**, which this slice takes.

### ⚠⚠ And a finding that corrects M8.3a's own headline

M8.3a concluded *"a single GPU stage inside a CPU pipeline is net-negative"*. Measured on the
transcendental probe: **GPU 4.06 ms against CPU 16.23 ms — 4× faster including a full round trip.**

**That conclusion was true of `ExposureStage` and false in general.** Exposure's compute is 0.04 ms,
so a ~6 ms transfer buries it; an Oklab round trip is heavy enough that the transfer is worth
paying on its own. **Whether one stage wins depends entirely on how expensive that stage is** —
which is a far more useful rule than the one M8.3a recorded, and it means the eventual stage
ordering should follow cost, not pipeline order.

## 2. The core move: a resident chain is itself one `PipelineStage`

`PipelineStage::apply(ImageBuf) -> ImageBuf` **forces a round trip by its signature** — it takes
CPU memory and returns CPU memory. That is exactly right as the seam and exactly wrong as the unit
of GPU work.

So residency introduces a **second, narrower trait**, and composes upward:

```
GpuChain  (implements PipelineStage)
  ├── upload once      ImageBuf -> GpuImage
  ├── encode stage 1   \
  ├── encode stage 2    >  one compute pass, no CPU involvement
  ├── encode stage N   /
  └── download once    GpuImage -> ImageBuf
```

The existing pipeline sees **one stage** that happens to do N stages' work, so nothing in
`pipeline` changes and the seam designed on day one still holds.

## 3. The three types

### `GpuImage`

A `wgpu::Buffer` plus width and height, living on the GPU across stages.

⚠ **In-place, not ping-pong.** Every per-pixel stage is in-place-safe because each invocation
touches only its own pixel. This is a **documented limit, not an oversight**: spatial stages
(M8.3c — local contrast, sharpening) read neighbours and will need a second buffer, so the type
must not preclude adding one.

### `GpuStage`

```rust
pub trait GpuStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn encode(&self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage);
    /// The CPU stage this one replaces, for the fallback path.
    fn cpu(&self) -> &dyn PipelineStage;
}
```

Deliberately **not** `PipelineStage`. It records a dispatch and never uploads or downloads — that
is the whole point, and the reason the two traits cannot be one.

⚠⚠ **`cpu()` is not decoration, and the spec review is what found it missing.** `apply` returns
`ImageBuf` with no failure channel, so a chain that fails on the GPU must produce the CPU answer —
and a `dyn GpuStage` exposing only `encode` has **nothing to fall back to**. Making each GPU stage
carry its own CPU twin is the same reasoning `GpuExposureStage` already applies to itself: *"holds
a CPU stage of its own rather than recomputing the gain, so the fallback path is the real thing and
cannot drift from it."* The alternative — a parallel `Vec<Box<dyn PipelineStage>>` on the chain —
is two lists that can disagree about their own length and order.

### `GpuChain`

Holds `Vec<Box<dyn GpuStage>>`, an `Arc<GpuContext>` and a persistent staging buffer. Implements
`PipelineStage`.

⚠ Its fallback runs **every stage's `cpu()` in order**, so a GPU failure yields exactly what the
CPU pipeline would have produced — not a partially-GPU result, which would be a third rendering
path nobody designed.

⚠ The staging buffer is **owned by the chain and reused**, which is where the measured 2.1×
lives. Allocating one per `apply` would give back most of it.

⚠ **`GpuExposureStage` is refactored to implement `GpuStage`**, and a `GpuChain` of exactly one
exposure must behave identically to today's stage — which its existing bit-exactness test already
asserts, so that test is the regression guard on the refactor.

## 4. The riskiest assumption, and the test that settles it

Successive dispatches in one compute pass write and then read the same storage buffer. **If
WebGPU does not guarantee memory visibility between dispatches in a pass, stage N+1 reads stage
N's unwritten data.**

⚠⚠ That failure would be **intermittent and resolution-dependent** — the worst kind, and exactly
the sort of thing that looks fine on a 64×48 fixture and corrupts a photograph. Reading the
specification is not sufficient: M8.3a's own record includes three hypotheses formed by reading
source that measurement then refuted.

**So the correctness test is also the barrier test:** a chain of three exposures at **+1.0, −0.5
and +0.25 EV** must equal a single stage at **+0.75 EV**. Gains multiply, so a missing barrier
gives a wrong product rather than a plausible image.

⚠ If it fails, the fix is one compute pass **per stage** rather than one for the chain — which
still keeps the buffer resident and still avoids N round trips, so the design survives. The spec
records that fallback so it is not rediscovered under pressure.

## 5. Substitution, and why it needs no change to `pipeline`

Substitution **cannot** walk a finished `Pipeline` and swap stages by inspecting them: a
`dyn PipelineStage` exposes only `name()` and `apply()`, so the parameters needed to build a GPU
equivalent are gone. But it does not have to inspect them — it can **rebuild from `Edits`**, which
the caller already holds:

```rust
// gpu-pipeline
pub fn accelerate(tail: Pipeline, edits: &Edits, ctx: &Arc<GpuContext>) -> Pipeline
```

It takes the built tail, replaces the stage named `hsl` with `GpuHslStage::new(&edits.hsl, ctx)`,
and returns the rest untouched. `Pipeline::into_stages` and `Pipeline::new` already exist, so this
needs **zero changes to `pipeline`** and the dependency direction is preserved: `gpu-pipeline`
depends on `pipeline`, never the reverse.

⚠⚠ **It is name-matching, which is stringly-typed, and that is the cost.** Two guards, both tests
rather than comments: the tail must contain **at most one** stage named `hsl`, and accelerating a
tail with no HSL must return it unchanged. The first holds because a mask's own HSL lives *inside*
`MaskStage`, which appears in the tail as a single stage named `mask` — a real property, and
exactly the kind a future refactor breaks silently.

### The alternatives, and why they lost

1. **A `StageAccelerator` trait threaded through `RenderSpec`** — `pipeline` would define a hook
   with no GPU types, which is a legitimate design and is probably right once *many* stages are
   accelerated. It loses today because `RenderSpec` is `Copy + PartialEq`, and an
   `Arc<dyn StageAccelerator>` is neither; making it non-`Copy` churns every builder and every
   call site for one stage's benefit.
2. **`gpu-pipeline` builds its own tail** — two orderings of the same chain that can disagree, the
   failure `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were each fixed for.

⚠ **When several stages are accelerated, revisit option 1.** Name-matching does not scale to a
dozen stages, and this spec would rather say so now than have it discovered as technical debt.

## 6. `GpuHslStage`

The most expensive stage in the tail — **+22 ms at fit** against a 12.7 ms neutral render, and the
one reported in use as *"a bit slow, but still usable"* at 1:1. That is why it is the stage worth
porting first, and §1's third finding is why porting it alone is worth doing at all.

⚠⚠ **The band logic never enters the shader.** `HueTable::build` already collapses eight bands and
their smoothstep blending into a 1024-entry table of `(Δhue, Δsaturation, Δluminance)` — 12 KB —
built **once per stage on the CPU** and uploaded as a storage buffer. So the shader does only:
ProPhoto→Oklab, chroma, `atan2` for the hue, a linearly-interpolated table lookup, the smoothstep
chroma fade, the small-angle rotation, Oklab→ProPhoto, and a clamp at zero.

⚠ The rotation uses the **same identity the CPU stage does** — `cos(hue)` and `sin(hue)` are
already in hand as `a/chroma` and `b/chroma`, so the chroma cancels and the rotation costs two
small-angle polynomials rather than two transcendental calls. Reimplementing it as a naive
`atan2`-then-`cos` round trip would be a *different* stage wearing the same name, and would drift
from the CPU one the moment either is tuned.

⚠ **Equivalence is a tolerance, not exactness** — unlike exposure. §1 measured this exact maths at
**0.0006 of an 8-bit level**, so the test asserts **below 0.01 levels**, which is ~16× headroom
over the measurement and still ~100× tighter than anything visible. ⚠ A loose tolerance here would
defeat the harness's whole purpose; if it ever has to be widened, that is a finding to investigate
rather than a number to adjust.

⚠ **The chroma early-out is a `return` on the CPU and a branch in the shader.** Below `EPS` the
CPU stage returns the pixel untouched; the shader must do the same rather than computing a
near-zero adjustment, or neutral pixels would pick up hue noise — the exact failure `CHROMA_FADE`
exists to prevent.

## 7. The app

A **Settings** checkbox, *Use the GPU for HSL*, defaulting **off**.

⚠ Off by default because this is the first GPU code any user would run, the CPU path is the oracle,
and a wrong default would make a rendering difference look like a Rustroom bug rather than a
setting. ⚠ Changing it re-renders, the way `apply_view_demosaic` does — a toggle whose effect only
appears after some unrelated edit reads as broken.

⚠ The accelerator is applied **between building the tail and running it**, in `render_engine`,
which is the one place that already holds both the `Edits` and the tail. ⚠ It must **not** be
applied to the prefix: HSL is a tail stage, and a prefix whose stages varied with a GPU setting
would make `PrefixKey` describe two different computations under one key.

## 8. Testing

1. ⚠⚠ **Three chained exposures equal one combined exposure** — the barrier test above, and the
   single most important assertion in this slice.
2. ⚠⚠ **A chain of 8 stages performs exactly ONE upload and ONE download.** Counted with an
   `AtomicUsize` incremented inside the chain's own upload and download helpers, read by the test —
   the same shape as `RenderEngine::requests_issued` and `ThumbLoader`'s extraction counter, both
   of which exist because a proxy observable is not proof. **Not timed:** a timing assertion is
   flaky under load, and this machine already has a background process at 100% CPU.
3. **A chain of one exposure is bit-identical to `GpuExposureStage` today**, which makes the
   existing bit-exactness test the regression guard on the refactor.
4. **An empty chain returns the input unchanged**, since a chain with no stages is a legitimate
   state and must not upload at all.
5. **A GPU failure inside a chain falls back to the CPU** for the whole chain, asserted against
   the CPU stages' own output — not merely "non-empty", which would pass against a fallback that
   returned the input.
6. **The persistent staging buffer is reused**, asserted by pointer identity across two `apply`
   calls: allocating per call would silently give back the 2.1×.

7. **`GpuHslStage` matches the CPU stage within 0.01 of an 8-bit level**, over a fixture spanning
   dark to above-white and including **near-neutral pixels** — which is where the chroma early-out
   and the fade live, and where a naive port goes wrong invisibly.
8. **A neutral pixel is returned untouched**, asserted exactly rather than within the tolerance:
   the early-out is a branch, so it either fires or it does not.
9. **The tail contains at most one stage named `hsl`**, which is what makes name-matching sound.
10. **Accelerating a tail with no HSL returns it unchanged** — the other half, without which a
    substitution that replaced the wrong stage would still pass test 9.

**Sabotage list**, each of which must fail a *named* test: split the chain into one pass per
dispatch *and* remove the barrier (test 1); upload per stage (test 2); allocate a fresh staging
buffer per `apply` (test 6); make the fallback return the input (test 5); drop the chroma early-out
from the shader (test 8); make `accelerate` replace the first stage rather than the named one
(test 10).

⚠ **The GPU tests skip without an adapter and print loudly**, as M8.3a's do — CI is Linux and may
have none. The cost recorded there still applies: **~42 s in a debug build against ~0.1 s in
release**, one-time per process. Iterate with `cargo test --release -p gpu-pipeline`.

## 9. What this deliberately does not do

1. ⚠⚠ **Masked HSL is NOT accelerated, and that is the sharpest limit here.** A mask builds its own
   sub-pipeline *inside* `MaskStage`, which the tail sees as one opaque stage, so `accelerate`
   cannot reach it. **That is precisely the case reported in use as slow** — *"masked HSL at 1:1 is
   a bit slow, but still usable"* — so this slice speeds up the global slider and leaves the
   complaint standing. Reaching inside `MaskStage` is its own design problem and belongs with
   M8.3d.
2. **Only HSL.** The other ten per-pixel stages wait for the next slice, where they are cheap once
   residency exists — and where the substitution mechanism will need rethinking (§5).
3. **No ping-pong buffer**, so spatial stages cannot join until M8.3c adds one.
4. **No zero-copy** — measured as unreachable; one memcpy into mapped memory is the floor.
5. **Off by default**, so nobody gets a rendering change they did not ask for.
6. **Nothing helps export**, still ~93% demosaic inside libraw and upstream of every stage `wgpu`
   would ever port.
7. ⚠ **The win is smaller than "HSL costs 22 ms" suggests.** A GPU chain of one stage still pays
   the full round trip — measured at ~6 ms with `write_buffer`, less with the persistent staging
   buffer — so the expected saving is a fraction of 22 ms, not all of it. **The honest figure is
   whatever the probe reports after implementation**, and the plan's last task measures it rather
   than this spec predicting it.
