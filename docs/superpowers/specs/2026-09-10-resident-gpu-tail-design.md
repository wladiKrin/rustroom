# M8.3b — residency and substitution

**Date:** 2026-09-10
**Status:** design approved, not yet implemented.
**Done when:** a chain of N GPU stages costs **one** round trip rather than N, proven by counting
uploads rather than by timing.

⚠⚠ **Like M8.3a, this slice has NO visible effect in the app** (user's call, 2026-09-10, choosing
"infrastructure only" over a scope that included two expensive stages and a Settings toggle). It
ports no new stage and wires nothing in. Launching Rustroom after it will show exactly nothing.

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

## 5. Substitution is deliberately deferred (user's call)

**Choosing GPU stages from `Edits` is not in this slice**, and the reason is structural rather
than a matter of effort.

Substitution cannot walk a finished `Pipeline` and swap stages: a `dyn PipelineStage` exposes only
`name()` and `apply()`, so **the parameters needed to build a GPU equivalent are gone**. It has to
happen at construction, and that leaves two options, both bad today:

1. **`pipeline` learns about the GPU** — wrong dependency direction, and it would put `wgpu` into
   the CLI, `export` and the 63 goldens, which §4 of M8.3a's spec exists to prevent.
2. **`gpu-pipeline` duplicates the tail's ordering logic** — two orderings that can disagree,
   which is the failure `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were each
   fixed for by deriving rather than storing twice.

So this slice delivers `GpuChain` as the **vehicle** for substitution and defers *choosing* to the
slice that has stages worth choosing. ⚠ When that slice comes, the question to answer first is
which of the two options above to take — and neither is obviously right, so it deserves its own
design rather than being decided in passing.

## 6. Testing

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

**Sabotage list**, each of which must fail a *named* test: split the chain into one pass per
dispatch *and* remove the barrier (test 1); upload per stage (test 2); allocate a fresh staging
buffer per `apply` (test 6); make the fallback return the input (test 5).

⚠ **The GPU tests skip without an adapter and print loudly**, as M8.3a's do — CI is Linux and may
have none. The cost recorded there still applies: **~42 s in a debug build against ~0.1 s in
release**, one-time per process. Iterate with `cargo test --release -p gpu-pipeline`.

## 7. What this deliberately does not do

1. ⚠⚠ **Nothing in the app uses it** — no Settings toggle, no substitution from `Edits`, nothing
   renders on the GPU. **Launching Rustroom after this shows no change at all**, exactly as after
   M8.3a. This was chosen explicitly over a scope that included two expensive stages and a
   toggle.
2. **No new stage is ported.** `GpuExposureStage` is the only one, refactored rather than added
   to.
3. **No ping-pong buffer**, so spatial stages cannot join until M8.3c adds one.
4. **No zero-copy** — measured as unreachable; one memcpy into mapped memory is the floor.
5. **Nothing helps export**, still ~93% demosaic inside libraw and upstream of every stage `wgpu`
   would ever port.
6. ⚠ **The 2.1× upload improvement is not a 2.1× render improvement.** It is 2 ms of a round trip
   that only matters once many stages share it, and with one stage in existence there is nothing
   yet to share it between.
