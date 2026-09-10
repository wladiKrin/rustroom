# M8.3c — spatial stages on the GPU

**Date:** 2026-09-10
**Status:** design approved, not yet implemented.

**Done when**, all three:
1. A `GpuChain` can run stages that need scratch memory — spatial stages read neighbours, which
   M8.3b's in-place-only `GpuImage` cannot serve.
2. `accelerate` merges **consecutive** GPU-capable stages into **one** chain, so a run shares one
   round trip.
3. `SharpenStage` runs on the GPU, matching the CPU within **0.01 of an 8-bit level** with masking
   both on and off.

---

## 1. What the measurements say, including one that shrank the scope

From `bench_preview` (2026-09-10), against a **12.7 ms** neutral tail at fit:

| stage | cost | |
|---|---|---|
| **sharpening 70** | **+7.1 ms** | this slice |
| texture + clarity | +10.8 ms | not this slice |
| *HSL, for comparison* | *+22 ms* | *got **2×** on GPU in M8.3b* |

### ⚠⚠ On its own, sharpening is break-even — the win is the RUN

M8.3b measured the round trip at ~6 ms and got 2× on a 21 ms stage. **A 7 ms stage against a 6 ms
round trip is break-even at best**, so porting sharpening as its own chain would be pointless.

What makes it worth doing is that `hsl` and `sharpen` can share **one** transfer. That is the
whole reason run-merging is in this slice rather than a later one: **the infrastructure is the
feature, and the stage is the thing that proves it.**

### ⚠⚠ And a correction to this spec's own first draft

The draft had colour grading in scope, on the reasoning that the tail order is
`… local_contrast → saturation → hsl → color_grading → sharpen`, so `color_grading` sits between
`hsl` and `sharpen` and would split the run.

**That reads the order as if it were static, and it is not.** Every tail stage is conditional on
`is_active()`, so on a photo with neutral colour grading — the common case — **`hsl` and `sharpen`
are already adjacent.** Run-merging alone reaches them. Colour grading is dropped from scope, and
this is recorded because the widening was nearly taken on a false premise.

⚠ The residual: **when colour grading IS active, the run splits** and the user gets two chains
instead of one. Stated in §8 rather than discovered.

## 2. Scratch planes, not chain-level ping-pong

M8.3b's `GpuImage` is deliberately in-place, and its doc says spatial stages "will need a second
buffer". They do — but **not as ping-pong**, and the distinction is worth getting right.

A separable blur runs `front → scratch → front`: the horizontal pass writes the scratch, the
vertical pass writes back. **The result lands where it started.** So:

> **Every stage still reads and writes `front`. `scratch` is transient *within* a stage.**

That keeps the chain's contract exactly as M8.3b left it — no bookkeeping about which buffer is
current, no rule about who owns what between stages, and no way for a stage to leave the chain
pointing at the wrong memory. True ping-pong would buy nothing here and cost all of that.

**`GpuStage::scratch_planes() -> usize`** declares how many single-channel `n`-float planes a
stage needs; `GpuChain` allocates the maximum across its stages, **once**, and passes them to
`encode` as a slice: `encode(&self, ctx, pass, img, scratch: &[wgpu::Buffer])`.

⚠ A slice rather than a count-and-index, so a stage cannot reach past what it asked for. ⚠ And the
planes are **shared between stages in a chain** — they are transient within a stage, so reuse is
safe and allocating per stage would multiply the memory for nothing.

⚠ Sharpening needs **4**: luma, the blurred luma, the difference, and the edge mask. That is
~26 MB alongside a 19.6 MB image at fit — allocated only when some stage asks for it, so an
HSL-only chain is unchanged.

⚠ `scratch_planes` defaults to **0**, so every M8.3b stage keeps working untouched.

## 3. Run-merging

`accelerate` walks the tail and groups **consecutive** replaceable stages into one `GpuChain`. A
stage it cannot replace ends the current run and passes through unchanged.

⚠⚠ **The second half of the test is what makes this meaningful.** "Consecutive stages are merged"
is passed by an implementation that merges *everything* regardless of position — which would put
CPU stages' work in the wrong order and corrupt the render. So the guard is a pair: consecutive
GPU stages become **one** chain, **and** a CPU stage between two GPU stages produces **two**.

⚠ Order is preserved absolutely. A chain runs its stages in the order they were added, and the
merged pipeline keeps every stage's position — this is a regrouping, never a reordering.

## 4. The substitution table

M8.3b's `accelerate` is `if s.name() == HSL { … }`. With two stages that becomes an if-chain, so it
becomes a table:

```rust
const REPLACEABLE: &[(&str, fn(&Edits, &Arc<GpuContext>) -> Box<dyn GpuStage>)] = &[…];
```

Adding a stage is one row.

⚠⚠ **This is NOT the `StageAccelerator` hook, which is still deferred.** M8.3b recorded that
name-matching "does not scale to a dozen stages" and that the hook — `pipeline` calling a trait at
construction — is the right answer then. A table is the smaller step that stops the if-chain
growing; it does not fix the underlying stringly-typed lookup. ⚠ The hook loses today for the same
reason it did in M8.3b: `RenderSpec` is `Copy + PartialEq` and an `Arc<dyn Trait>` is neither.

## 5. The setting has to be renamed, and that is a migration

M8.3b's setting is `Settings::gpu_hsl`, labelled **"HSL / Use the GPU"** with a hint that says
*"Runs the global HSL sliders on the graphics card."* **Adding a second stage makes the field name,
the label and the hint all wrong**, and a setting that silently governs more than it claims is the
class of thing this project has repeatedly had to correct — the `Effects` edit group that held only
a vignette, and the "Reset crop" hover text that enumerated what it reset.

So: **`gpu_hsl` → `gpu_tail`**, labelled **"Use the GPU"** under a *Rendering* row, with a hint
naming what it covers and what it does not.

⚠⚠ **Renaming a serialised field is a migration, not a rename.** `Settings` carries a
container-level `#[serde(default)]`, so without an alias an existing `settings.json` holding
`"gpu_hsl": true` would silently fall back to `false` — the user's choice discarded with nothing
to notice it. **`#[serde(alias = "gpu_hsl")]`** is required, and it needs its own test: the
`EditGroups::effects` → `vignette` rename set the precedent, and its alias test exists because the
same trap was live there.

⚠ The rename reaches `FrameSpec::gpu_hsl`, `Session::gpu_hsl` and `apply_gpu_hsl` too; the compiler
names every site.

## 6. `GpuSharpenStage`

The CPU stage, per pixel: luma → Gaussian blur → `d = luma − blurred` → optionally a mask
(`|d|` blurred, then a smoothstep) → a scalar RGB gain with a halo suppressor and a near-black
guard.

**On the GPU that is 5–9 dispatches**, all inside the one compute pass M8.3b proved carries a
memory barrier between dispatches:

| | |
|---|---|
| luma | 1 |
| Gaussian, horizontal + vertical | 2 |
| `d = luma − blurred` | 1 |
| *masking only:* `\|d\|`, Gaussian ×2, smoothstep | 4 |
| apply the gain | 1 |

⚠⚠ **The kernel weights are computed by the CPU and uploaded**, not reimplemented — `spatial::
gaussian_kernel` becomes `pub` (a narrow re-export by name, the rule `HueTable::entries` and the
hue LUT already follow, never `pub mod spatial`). Two implementations of a Gaussian is how the two
come to disagree about a σ nobody re-derives.

⚠⚠ **Edge handling must replicate exactly as the CPU does**: `(x + i).saturating_sub(r).min(w-1)`.
In WGSL those indices are **unsigned**, so `x + i - r` underflows to a huge number instead of
clamping at zero — the trap here, and it would show as a bright or dark fringe down the left and
top edges only.

⚠ **Masking 0 skips the whole branch**, as on the CPU: it is both the cheap path and what avoids
`smoothstep(0, 0, ..)` being degenerate. So the shader has a real branch and the tests must cover
both sides of it.

## 7. Testing

1. **Equivalence within 0.01 levels, masking OFF** — the plain unsharp path.
2. **Equivalence within 0.01 levels, masking ON** — a different code path with a second Gaussian
   and a smoothstep. ⚠ One without the other tests half the stage.
3. ⚠⚠ **A hard edge across the frame**, not a smooth ramp. Sharpening is *about* edges, and a
   ramp would let a wrong kernel or wrong edge replication pass — the fixture must contain what
   the stage acts on, the lesson the M8.4 demosaic probe learned by landing on road-sign
   lettering.
4. ⚠⚠ **The left and top edges are asserted specifically**, because that is where unsigned
   underflow in the sample index shows and nowhere else.
5. **`hsl` + `sharpen` in one chain performs exactly ONE round trip**, counted per chain.
6. **Consecutive GPU stages merge into one chain.**
7. ⚠⚠ **A CPU stage between two GPU stages produces TWO chains** — without this, an implementation
   that merged everything regardless of position would pass test 6 while corrupting the render.
8. **A chain with no scratch-needing stage allocates no scratch**, so an HSL-only chain costs
   exactly what it did in M8.3b.

**Sabotage list**, each failing a *named* test: clamp the sample index with `min` only, dropping
the lower bound (test 4); reimplement the kernel weights instead of uploading them (tests 1–2);
skip the mask Gaussian (test 2); merge non-consecutive stages (test 7); allocate scratch
unconditionally (test 8).

⚠ GPU tests **skip without an adapter** and print loudly, as M8.3a and M8.3b's do. Iterate with
`cargo test --release -p gpu-pipeline`; a debug build pays ~42 s once for the adapter.

## 8. What this deliberately does not do

1. ⚠⚠ **Sharpening alone is break-even** (+7.1 ms against a ~6 ms round trip). **The win is the
   run**, and if a photo has colour grading active the run splits into two chains and most of it
   is lost. That is the honest shape of this slice.
2. **Local contrast is NOT ported.** It needs `gaussian_blur` *and* `wide_blur` (three iterated
   box blurs) *and* `guided_filter` (four box blurs plus arithmetic) — three blur kinds, each
   needing its own equivalence test. Its own slice.
3. **Colour grading is not ported**, so an active grade splits the run — §1.
4. **Masked sharpening is not accelerated**, for the same reason masked HSL is not: a mask's
   sub-pipeline lives inside `MaskStage`, which `accelerate` sees as one opaque stage.
5. **The `StageAccelerator` hook is still deferred** — a table is not a fix for stringly-typed
   lookup.
6. **Still off by default**, under **one** renamed toggle rather than one per stage — it is a
   single decision about whether the tail runs on the GPU. ⚠ Anyone who had `gpu_hsl` on keeps it
   on, via the serde alias in §5.
7. **Nothing helps export**, still ~93% demosaic upstream of every stage `wgpu` would port.
