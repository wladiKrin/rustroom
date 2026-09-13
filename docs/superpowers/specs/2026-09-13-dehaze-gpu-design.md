# M8.3o — dehaze on the GPU, and a seam for the prefix

**Date:** 2026-09-13
**Status:** design

Dehaze as compute shaders — the first **prefix** stage to move, and therefore the first that needs
a seam `accelerate` does not provide.

## Why this one, measured before it was chosen

Re-scanned today across 40 sidecars / 40 copies. **Dehaze is the most-used unported stage in this
catalogue**, and by more than its own margin — everything else unported is 0 or 1:

| unported stage | copies using it |
|---|---|
| **dehaze** | **2** (70.0 and 100.0) |
| noise reduction | 1 |
| local contrast | 1 |
| vignette | **0** |
| creative look | **0** — `null` in every sidecar |

⚠ That last row corrects something said out loud one message earlier: *"what is left that can still
split the tail is the vignette and the creative look"*. Structurally true, and worth **nothing**
here — neither is used by anything.

And the user's own settings make it bite: **`drag_quality: Full` with `gpu_tail: true`**, so a
fitted photo on a large window renders at a native rung while dragging. Measured at 6.5 Mpx
(`bench_preview`, labels now derived from the steps rather than hardcoded):

| | ms |
|---|---|
| `dehaze` alone, whole buffer | **100.9** |
| **dragging Dehaze** | **134.3 — 7.5 fps** |
| `noise_reduction` alone | 87.7 |
| dragging NR | 220.7 |
| a tail slider (nothing in the prefix re-runs) | 35.8 |

## ⚠⚠ The structural fact that shapes everything: the prefix has no seam

`accelerate` receives the **tail**. The prefix is a `Vec<PrefixStep>` built by
`build_prefix_steps`, and each step's output is cached as a **CPU `ImageBuf` checkpoint**. So a GPU
prefix stage must upload *and* download for one stage — the M8.3a shape that measured net-negative.

⚠ It is net-positive here only because dehaze is expensive: **100.9 ms of CPU against a 26.0 ms
round trip**, measured today rather than assumed.

## ⚠⚠ Per step, NOT chained — and the measurement reverses the intuition

The obvious design chains the two prefix stages so the transfer is paid once. It is the wrong one,
and the reason is that chaining destroys the **post-NR checkpoint**, which exists precisely so a
Dehaze drag resumes after NR.

Priced from the components measured below, for a **Dehaze drag** — the interaction that matters,
since dehaze is used twice as often as NR:

| | dehaze drag | NR drag |
|---|---|---|
| today (CPU) | 134.3 ms | 220.7 |
| **per step, checkpoint kept** | **~92** | ~139 |
| chained, checkpoint lost | ~113 | ~113 |

**Per step wins the case that matters and costs far less machinery**: the existing cost gate, a
one-stage `GpuChain`, and `PrefixCache` completely untouched. ⚠ Chaining wins an NR drag, and that
is recorded rather than hidden — it becomes the right answer if NR is ever ported too *and* the
checkpoint's value is re-measured.

⚠⚠ Note what chaining would **not** have cost: even with the checkpoint lost, a chained GPU prefix
(~113 ms) still beats today's CPU dehaze drag (134.3). The checkpoint question is therefore about
*how much* to win, not about whether — which is worth knowing before anyone re-opens it.

## The GPU side, priced from measured components

`box_probe` at 3123×2085, the same instrument M8.3n's threshold came from:

| component | measured |
|---|---|
| float round trip (transfer only) | **26.04 ms** |
| dehaze's guided filter, r=31 ×6, running body | **21.96** |
| dehaze's dark channel, r=10 ×1 | **4.49** |
| per-pixel work (normalize, distance, coarse, final) | ~3–5 (estimated) |

→ **~56 ms against 100.9 on the CPU, about 1.8×.** ⚠ Stated plainly: this is a **modest** win, and
roughly half of it is the transfer rather than the arithmetic. A Dehaze drag goes **134 → ~92 ms**,
7.5 → 11 fps.

⚠ The per-pixel row is the one estimate in that table, and this project's record on composing
component measurements is poor — M8.3g over-predicted a whole frame by ~2×, M8.3f was right in
magnitude and wrong by 8× in attribution. **The plan's last task re-measures the real thing.**

### ⚠⚠ Two of dehaze's three primitives are already built

- **The guided filter is box blurs**, and M8.3n shipped both bodies. ⚠ Dehaze's is **cross**-guided
  (`p` is the transmission map, `guide` is luma), so M8.3n's self-guided shortcut does **not**
  apply and all six statistic blurs are needed — which is what the 21.96 ms above prices.
  ⚠ `REFINE_RADIUS_FULL = 60` scales to **r=31** at a native rung, above the crossover, so it takes
  the **running** body — the one M8.3n found is flat in the radius.
- **The airlight stays on the CPU entirely.** It is already estimated on a **fixed 256 px
  downscale** — a deliberate decision, so the estimate is render-scale-independent and the image
  does not shift on zoom — which makes it negligible *and* makes the two paths identical **by
  construction** rather than by comparison. It arrives as three uniforms.
- **`min_filter` is the one new primitive.** ⚠⚠ A sliding-window **minimum is not invertible**, so
  the running-sum trick that saved Clarity has no analogue: there is only the naive O(r) gather.
  That is affordable here only because the radius is small — `PATCH_RADIUS_FULL = 20` scales to
  **r=10**, measured at **4.49 ms**, where Clarity's r=40 cost 46.9. ⚠ Its border rule is
  `box_blur`'s, not `gaussian_blur`'s: the **window clamps** rather than the samples, which is
  exactly why `spatial::min_1d` uses a monotonic deque and not van Herk / Gil-Werman.

## Design

`gpu_pipeline::accelerate_prefix(steps, edits, scale, ctx) -> Vec<PrefixStep>` — the `accelerate`
shape, one level up:

- substitute a step's `stage` by **name**, leaving its `key` untouched;
- gate each step on `GpuStage::cpu_cost()` against the existing `TRANSFER_COST`;
- fail toward the CPU everywhere — no adapter, no match, too cheap.

⚠⚠ **The `key` must not change, and that is the whole reason this is safe.** `PrefixStepKey` is
derived from the **edits**, not from the stage, so a GPU substitution is invisible to
`PrefixCache`: a checkpoint written by the CPU path is a legitimate hit for the GPU path and the
reverse. ⚠ It also means `gpu_tail` must **not** enter any key — the same rule the resampler
follows and HSL deliberately does not.

⚠ The hook goes where `render_cached` calls `build_prefix_steps` (`render_engine.rs:1080`), beside
the existing `accelerate` call, and is offered only when the setting is on.

## Equivalence

Bounded in **8-bit levels**, not bit-identical: dehaze is full of divides and multiply-adds, so it
inherits both FMA contraction (M8.3m) and the division class M8.3n found.

⚠ Fixtures must carry **values above 1.0 and below 0** — the working space is scene-referred — and
must contain **real haze structure**, since dehaze's whole behaviour is a depth gradient. The
stage's own tests already record what such a fixture needs and why three earlier attempts were
wrong: the dark-channel prior must actually hold, the transmission must reach near zero somewhere
or the airlight is unobservable, and the guided refinement's reach is **twice its radius**.

## Testing

1. `min_filter` against `spatial::min_filter` directly, at r either side of the radii that occur,
   plus **a constant plane**, which is the only thing that separates the two border rules.
2. The cross-guided filter against `spatial::guided_filter` — ⚠ with `p != guide`, which is the
   path M8.3n's self-guided tests structurally cannot reach.
3. The stage against `DehazeStage`, both signs of the slider, on a fixture the CPU stage moves.
4. **The airlight arrives unchanged** — a test that the GPU stage is constructed with the same
   three numbers `dehaze::airlight` produces, since a second estimate would be a different
   photograph rather than an error.
5. ⚠⚠ **The cache is unaffected**: a checkpoint written by one path must be a hit for the other.
   That is what `PrefixStepKey` being edits-derived buys, and it needs a test or it is a claim.
6. The 63 goldens must not move — the CLI and export never call this.

## Not in scope

**Noise reduction**, though the seam serves it and it is the obvious follow-up: it is 1 of 40 here
against dehaze's 2, and one stage first is how M8.3a proved the tail seam. **Chaining prefix
steps**, for the reason above. **The masked dehaze**, which runs through `capture_projection` in
the same prefix and would come along only once the seam is proven.
