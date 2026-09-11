# M8.3j — a mask's sub-stages on the GPU

**Status:** design approved 2026-09-11. Branch `m8.3j-mask-substages`.

> ⚠ The numbering has shifted five times in Phase 8. M8.3i took the slot this milestone was
> originally scoped into, so **the GPU mask loop is M8.3j**. The rule is unchanged: whatever ships
> next takes the next letter.

## 1. What this is, and what the measurement says it is not

M8.3i instrumented `MaskStage::apply` with four phase timers and found that **"the mask loop" is
not one thing.** Per mask, rung 3200, 3123×2085 (6.5 Mpx), Apple M2, release, best of 5:

| mask | coverage | clone | sub-stages | blend | total |
|---|---|---|---|---|---|
| exposure, gradient | 0.00 | 2.06 | 3.8 | **11.1** | 17.0 |
| **HSL, gradient** | 0.00 | 2.06 | **68.8** | 11.1 | 82.0 |
| exposure+contrast+HSL, gradient | 0.00 | 2.1 | 67.5 | 11.5 | 81.2 |
| exposure, colour range | 0.00 | 2.3 | 4.0 | **61–120** | 67–128 |
| exposure, brush (plain, uncached) | 26.6 | 2.3 | 4.0 | 9.3 | 42.4 |
| **the catalogue's one real mask** | **228 → ~80** | 2.1 | **3.8** | 9.7 | **236 → ~140** |

**This milestone attacks exactly one column: the sub-stages.** It is the column that dominates
**masked HSL**, which is the case reported in use as slow — *"a bit slow, but still usable"* at
M7.2, and measured at **1 masked HSL = 120 ms and 3 masked HSL = 290 ms (3.4 fps)** at rung 3200
against a whole typical frame at 33 ms.

⚠⚠ **It buys nothing for the other three columns, and saying so up front is the point.** A colour
range's 61–120 ms blend is the range-mask coverage cache's job (`docs/BACKLOG.md`); an auto-masked
brush's coverage was M8.3i's; a gradient's blend is untouched by anything here.

## 2. The structural difficulty, and the fact that dissolves most of it

`MaskStage.stages` is an ordinary `Vec<Box<dyn PipelineStage>>`, built by `adjustment_stages`.
`GpuChain` **already implements `PipelineStage`**. So a mask's sub-stage run is shaped exactly like
a tail, and the acceleration machinery already fits it — `accelerate` simply cannot *reach* it,
because `pipeline` sits below `gpu-pipeline` and the tail sees `MaskStage` as one opaque stage
named `"mask"`.

⚠ Of a mask's ten possible sub-stages **five are ported** (exposure, tone_regions, hsl,
color_grading, sharpening) and **five are not** (white balance, contrast, tone curve, local
contrast, saturation). This ships partial by construction, and M8.3e plan 2 is the groundwork that
would raise it above half.

### 2.1 The trap that any design here has to answer

Every `REPLACEABLE` builder reads the **global** `Edits`:

```rust
("hsl", |e, _, _, _, ctx| Box::new(GpuHslStage::new(&e.hsl, ctx))),
```

Pointed at a mask, that would apply the **global** HSL values inside the mask. Sound today only
because the tail's stages were built from those same `Edits`. **The failure is a plausible
photograph, not an error** — the family this file records more than any other.

## 3. The seam

`pipeline` gains **one defaulted trait method**, and that is the whole of its public change:

```rust
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, image: ImageBuf) -> ImageBuf;

    /// Offer this stage's inner sub-stages for acceleration. Default: this stage has none.
    fn accelerate_inner(&mut self, _f: &SubStageAccelerator<'_>) {}
}

/// Given a stage list and the adjustments it was built from, return a possibly-accelerated list.
pub type SubStageAccelerator<'a> = dyn Fn(Vec<Box<dyn PipelineStage>>, &LocalAdjustments, RenderScale)
    -> Vec<Box<dyn PipelineStage>>
    + 'a;
```

⚠ The alias carries a lifetime because the closure `gpu-pipeline` supplies **borrows** — it holds
the outer `ProfileRendering<'_>` and the `&Arc<GpuContext>`. A `'static` alias would force those to
be owned, and `ProfileRendering` is a borrow by construction.

⚠ **`&mut self`, not `self: Box<Self>`.** `accelerate` already owns each box from `into_stages()`,
so `MaskStage` can `mem::take` its `stages`, hand them over and put the result back — no cloning,
no reconstruction, object-safe, and a default body that every other stage inherits untouched. A
`self: Box<Self>` receiver is object-safe but cannot carry a default body, because returning `self`
as `Box<dyn PipelineStage>` needs an unsize coercion and therefore `Self: Sized`.

⚠ `MaskStage` re-acquires two fields it does not currently keep: the `LocalAdjustments` its
sub-stages were built from, and the `RenderScale` (`GpuSharpenStage`'s sigma is in
**full-resolution** pixels and only the scale says what that is at this rung). The doc comment
saying the adjustments are *"deliberately not retained"* is rewritten rather than left to
contradict the code.

## 4. The projection, made total by the compiler

`gpu-pipeline` owns the `LocalAdjustments → Edits` mapping; `pipeline` stays unaware that GPU
exists. It is written as an **exhaustive destructuring**:

```rust
let LocalAdjustments {
    // projected — a GPU stage reads each of these
    exposure, highlights, shadows, whites, blacks, hsl, color_grading, sharpening,
    // deliberately not projected — no GPU builder reads them
    noise_reduction: _, dehaze: _, white_balance: _, contrast: _,
    tone_curve: _, saturation: _, vibrance: _, local_contrast: _,
} = a;
```

⚠⚠ **Adding a field to `LocalAdjustments` is then a compile error here**, rather than an
adjustment that silently renders as its default. That is `source_for`'s *"exhaustive on purpose — a
catch-all here is a trap"* applied to the thing that would otherwise produce a plausible picture.

⚠ `display_rendering` and `output_transform` never occur in a mask's sub-stage list, so their
builders' `ProfileRendering` and `RenderSpec` are passed straight through from the outer call and
are inert here.

## 5. Recursion depth is one, structurally

`accelerate` calls `accelerate_inner` on every stage it walks, passing a closure that projects,
then runs the **same** run-detection and the **same** cost gate on the mask's stage list. The inner
call passes a **no-op** accelerator.

That is not a depth limit bolted on: `adjustment_stages` is exhaustive over `LocalAdjustments`,
which has no `masks` field, so **a mask's sub-stages can never contain a mask.** Pinned by a test
asserting a mask's sub-stage names never include `"mask"`.

`MaskStage` is not in `REPLACEABLE`, so it still ends the current run and keeps its position —
unchanged behaviour, and the reason a masked photograph's own tail is unaffected.

Two properties fall out free, and both get a test rather than a comment:

- **The prefix is never accelerated.** `accelerate` is only ever called on the tail (`render_engine`
  passes `tail`); `MaskStage::new_capture` masks live in the prefix. `PrefixKey` keeps describing
  one computation.
- **A mask's chain is never trailing**, so it takes the float round trip and `TRANSFER_COST = 27.0`
  applies as measured. The inner `accelerate` call can never see `output_transform` as its last
  stage, so it returns `(Pipeline, None)` and the byte path is unreachable **by construction**.

## 6. The cost gate does the right thing per mask, unchanged

| a mask carrying | sub-stage cost | verdict |
|---|---|---|
| `hsl` | 60.0 | **chained** — clears 27.0 |
| `color_grading` | 48.0 | **chained** |
| `sharpening` | 54.0 | **chained** |
| `exposure` only — the catalogue's real mask | 1.3 | **refused**, correctly |
| `exposure + tone_regions` | 19.3 | refused |

⚠ So **the catalogue's one masked photograph pays nothing and gains nothing**, which is the honest
headline of this milestone and is recorded as such rather than discovered later.

## 7. Testing — and what the goldens cannot see

⚠⚠ **`grep -rln gpu tests` returns nothing: the 63 goldens never touch the GPU.** They are a
regression guard on the **CPU** path — they prove the defaulted no-op and the re-added fields
changed nothing — and they are **blind to every line this milestone adds.** Recorded up front,
because M8.3i's sharpest finding was a test that named something it could not detect.

The guards that can actually see it:

| guard | what it catches |
|---|---|
| **equivalence:** a masked render CPU vs GPU through `compare`, on a fixture with **every projected field non-default** | a forgotten or mis-mapped projection field — the plausible-picture bug |
| **decision:** a mask carrying only `exposure` produces no chain; one carrying `hsl` produces one | the gate reaching the wrong verdict inside a mask |
| a mask's sub-stage names never include `"mask"` | the recursion-depth assumption |
| the prefix pipeline contains no `gpu_chain` | prefix acceleration, which would corrupt `PrefixKey` |
| an accelerated mask's stage list is **shorter** than the CPU one | the hook silently doing nothing — every equivalence test passes against a no-op |

⚠ That last row is the W5 lesson (creative LUTs shipped fully tested and 100% dead): every
equivalence test here compares an accelerated render against a CPU one, and a hook that never fired
makes the two trivially identical. Something must assert the chain was **built**.

## 8. Acceptance

1. **`mask_breakdown` on the masked-HSL gradient** for the headline. Predicted: sub-stages
   68.8 → ~27 ms (the float round trip, with the kernel close to free once resident — M8.3d's
   finding), mask total **82 → ~42 ms**.
2. **A real photograph, masked by hand.** Paint an HSL-carrying mask onto a real `.RAF` and price a
   drag with `RUSTROOM_DEBUG_MASK` in the real engine. That is what makes it *confirmed by use*
   rather than *confirmed by benchmark*.

⚠ `probe_rt` is re-run only if the upload, the dispatch count or the download changes. Nothing here
does — this adds a chain in a new *place*, not a new shape of transfer.

## 9. Accepted caveats, stated in advance

1. **Five of ten sub-stages ported**, so a mask carrying white balance, contrast, a tone curve,
   local contrast or saturation keeps those on the CPU and splits its run around them.
2. **`image.clone()` (~2 ms) is wasted** when the whole sub-stage run becomes one chain — the clone
   is uploaded and immediately discarded. Not reclaimed in this slice; it is ~2 ms of ~42.
3. **N masks means N round trips**, un-batchable: each mask's sub-stages run on a clone of the
   *current* image and each blend is CPU and sequential.
4. **Nothing for a colour range's 61–120 ms blend**, nothing for brush coverage, nothing for a
   gradient's blend.
5. **No gain on the catalogue's only masked photograph**, by design — the gate refuses it.
6. One machine, one adapter (Apple M2, Metal), one rung.
7. **`gpu_tail` is still off by default**, so a fresh install sees no change.
