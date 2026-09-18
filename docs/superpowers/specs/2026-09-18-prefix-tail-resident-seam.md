# A GPU buffer resident across the prefix seams

**Date:** 2026-09-18
**Status:** approved
**Item:** `docs/BACKLOG.md` → *"A GPU buffer resident across the prefix→tail seam"*

## What this is

Consecutive GPU-accelerated prefix steps, and the last one and the tail's first chain, currently
hand pixels to each other **through host memory**. Each producer downloads its result and each
consumer uploads the same bytes back. This keeps the device buffer instead.

The mechanism is M8.13's and is already public: `GpuChain::offer_output()` on the producer,
`GpuChain::allow_shared_input()` on the consumer, through a `SharedPlane<GpuImage>`.

## What it is worth — measured before designing, not estimated

`DSCF6509.RAF`, 6246×4170 (26.0 Mpx, a 312.5 MB plane), quiet machine (load 2.39), warm frames,
edits = NR + dehaze in the prefix and a six-stage tail. `crates/app/examples/probe_prefix_tail_seam.rs`.

| | measured |
|---|---|
| tail chain handed a resident plane (direct A/B) | **17.9–23.5 ms saved** |
| both seams, same shape | **~36–47 ms** |
| whole-frame render, excluding the probe's A/B double-run | **~790 ms** |
| **share of a whole-frame render** | **4.5–6%** |
| interactive drags | **0** |

⚠⚠ **The two downloads are NOT in scope and are the larger half.** `tovec` is 27.4–27.6 ms (NR) and
24.8–31.4 (dehaze) — another ~52–59 ms. Removing those means `PrefixCache` holding a `GpuImage`,
which changes what a checkpoint *is*. **Priced at 11–13% and deliberately not taken** (user's call,
2026-09-18). Do not let this spec grow into it.

⚠⚠ **Interactive drags gain nothing, and that is not a defect.** `render_engine.rs:1190` keeps
`gpu_prefix_steps` at 0 on the region-scoped path — *"a scoped chain is the CPU's"* — so there is
no GPU prefix step, and therefore no seam, on the path M8.12 made the interactive one. **This is an
export and cold-whole-frame win.** Anyone re-reading this looking for a drag improvement should
stop here.

⚠ **Unlike M8.13, the device-side copy is not a significant share.** `memcpy` goes 8.6–9.5 → 0.22–0.25
while `submit` barely moves (172 → 170), because six dispatches over 26 Mpx dominate the submit. The
saving is the host memcpy plus allocation overhead. **Do not requote M8.13's "the device half was as
large as the host half" here** — it was measured on a 78 MB plane behind a much smaller dispatch.

## Design

### 1. A second slot, not the mask's

`GpuContext` gains a **`prefix_plane`** slot beside the existing `shared_plane`.

⚠⚠ **This is not tidiness, it is a guard this project already paid for with a sabotage.**
`the_outer_tails_own_chain_never_takes_a_masks_leftover_plane` exists to stop the outer tail's chain
taking a plane a mask left behind: a mask whose own chain sits behind a CPU stage leaves its deposit
in the slot, and the outer tail's chain runs *before* any mask, so on the next frame it would find
that plane at matching dimensions on the same thread. Letting the tail take from the mask's slot
would contradict that guard directly.

With two slots, a mask's leftover cannot reach a prefix consumer and a prefix offer cannot reach a
mask's sub-chain **by construction** rather than by checking. `offer_output` and
`allow_shared_input` each take which slot they mean.

### 2. A CPU step in between must clear

Consecutive accelerated steps are safe by construction: step *i+1*'s input **is** step *i*'s output.
A CPU step running between two accelerated ones breaks that, and the stale plane is the wrong pixels
at matching dimensions on the same thread — **a plausible photograph, not an error**.

`render_engine`'s loop already branches on `match accelerated`; the `None` arm clears the prefix
slot. The knowledge lives there and nowhere else.

⚠ A region crop needs no rule: `img.crop(..)` changes the dimensions, so the take misses and the
consumer uploads. A crop that happens to be the whole frame has identical pixels anyway. **Failing
toward an upload is always correct** — the seam's own rule, and `GpuBrushRaster`'s.

### 3. `accelerate_prefix_stage` returns `Option<GpuChain>`

It currently returns `Option<Box<dyn PipelineStage>>`, which hides the chain from the only caller
that knows the step order. `GpuChain` already implements `PipelineStage`, so `.apply()` is unchanged
and the caller boxes it where a box is wanted.

The step→tail seam needs `accelerate` to be told the prefix offered. That is an 8th argument, and
**clippy's `too_many_arguments` fired at exactly 8 on this function's sibling** — so its arguments
are grouped, the way `SeamFacts` was, rather than the lint being silenced.

## What is deliberately NOT here

- **`PrefixCache` holding a `GpuImage`.** Priced at 11–13%, declined. It would make
  `MID_BUDGET_BYTES` describe VRAM and force an answer from the region crop, the brush cache's
  sizing and `render_cached`'s tail seeding.
- **Fusing the last prefix step into the tail's chain.** It destroys the checkpoint, reversing
  M8.3o's measured decision without the measurement having changed.
- **Anything on the region-scoped path.** There is no seam there.

## Testing

**Guards, each pinned by a sabotage that must fail it:**

| guard | sabotage |
|---|---|
| a mask's leftover is never taken by a prefix consumer | cross-wire the two slots |
| a prefix offer is never taken by a mask's sub-chain | cross-wire the two slots |
| a CPU step between two accelerated ones clears | delete the `None`-arm clear |
| the take actually fires | make it always miss — a fixture check must fail, not a pixel test |

**Equivalence:** a two-accelerated-step prefix plus tail must render the same picture with taking on
as with it off.

⚠⚠ **The band is measured at both ends, never borrowed.** M8.13's first attempt asserted `< 0.02`
levels copied from a one-stage chain's test and failed at 0.088 for a reason that had nothing to do
with the hand-off. Run the comparison with the take forced to miss, record both numbers in the test,
and set the bound from them.

⚠ **A counter is not a picture.** `SharedPlane::takes()` says a plane moved; it cannot say it was
the right plane. Every slot guard needs the counter **and** the equivalence.

## Done when

- Both seams hand over, and `RUSTROOM_DEBUG_XFER` shows `took=true` with `memcpy` at ~0.2 ms on the
  consumer side of each.
- The probe reports a saving in the measured 36–47 ms family on a quiet machine.
- FMT=0, CLIPPY=0, TEST=0, each read from its own command; 74 goldens unmoved.
- Confirmed by use before merging.
