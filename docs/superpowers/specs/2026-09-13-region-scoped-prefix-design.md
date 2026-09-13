# M8.6 — a region-scoped prefix for the analytic resamplers

**Status:** design, 2026-09-13. Written against a measured report — *"it's slow on every frame of
the straighten drag, but especially when zoomed in"* — and it **displaces item 3 of the agreed
order** rather than following it.

**One line:** during a Straighten or Perspective drag the prefix's last step produces only the
sub-rect the tail is about to crop to, which at 1:1 is **1.26 Mpx instead of 26** — a drag frame
**144.4 → ~26 ms**, with the picture bit-identical.

---

## 1. The measurement, and it moved the milestone

`crates/app/examples/probe_resamplers.rs` (kept, extended here with drag rows), `DSCF6296.RAF`,
Catmull-Rom, lens profile matched, 8 threads. ⚠ Load ~2.9, so **not a quiet machine**.

A drag re-runs only its own prefix step and everything downstream; everything upstream stays
cached, which is what these rows price.

| what a drag actually costs | fitted rung (6.5 Mpx) | **1:1 (26.0 Mpx)** |
|---|---|---|
| `geometry` — the step Straighten invalidates | 33.4 ms | **131.7** |
| the tail, on a 1400x900 region | 8.4 | 19.8 |
| **a Straighten drag frame** | **40.6 ms / 25 fps** | **144.4 ms / 6.9 fps** |
| *a lens-correction drag, for contrast* | *259.9* | *1002.1* |
| *a tail slider (exposure)* | *8.4* | *19.8* |

**144 against 41 is 3.6x, which is the pixel ratio between the two rungs** — so the cause is the
buffer size and not something incidental to zooming.

⚠⚠ **AND LENS CORRECTION CONTRIBUTES EXACTLY ZERO TO A STRAIGHTEN DRAG.** Its checkpoint is keyed
on edits the drag never touches, so it stays valid throughout. **Item 3 of the agreed order —
port lens correction to the GPU — does not touch this symptom at all.** It is the right answer to
a cold render and a rung change (that 1002.1 ms row) and the wrong answer to *"the straighten drag
is slow"*. That is M8.3q's lesson arriving a second time: **the row that matters is a drag, not a
stage**, and the next-action block did not carry it.

⚠ M8.5 already took this gesture **~4x** — geometry was sequential, so a drag frame was ~600 ms
before it — and it is still 6.9 fps. This is the next lever, not a retraction of that one.

---

## 2. The structural cause

**The prefix runs whole-buffer while the tail renders a region.** `render_engine.rs:1085` states it
plainly: *"The prefix never depends on the region, so the whole chain is built and cached without
one."* At 1:1 geometry therefore resamples all 26 Mpx to display a 1400x900 slice — **about 20x
more pixels than are on screen**.

⚠⚠ **That is correct for caching and worth nothing during a drag.** Keeping the region out of the
key is what makes panning free: a pan re-runs only the tail and invalidates no checkpoint. But the
step being dragged is invalidated **every frame by construction**, so for that one step the cached
whole-buffer output is never reused — it is computed and thrown away.

---

## 3. The mechanism

Both stages are **gathers**: an output pixel computes its own source coordinate and reads the whole
source. So producing a sub-rect of the output needs **no pre-image analysis, no margin and no
bounds work** — iterate fewer output pixels against the same source buffer.

One defaulted `PipelineStage` method, the fifth on that trait:

```rust
fn apply_region(&self, _src: &ImageBuf, _rect: OutRect) -> Option<ImageBuf> { None }
```

- ⚠⚠ **It borrows rather than consuming, and that is load-bearing rather than tidy.** `apply`
  consumes its input — the ownership that keeps the GPU seam clean — so the normal path clones the
  26 Mpx checkpoint, **measured at 8.4 ms**. That clone does not shrink with the region and would
  be roughly 60% of the projected frame.
- ⚠⚠ **`Option`, so every other stage declines BY CONSTRUCTION.** Dehaze, noise reduction, spot
  removal and the masked capture corrections inherit the default and cannot be region-scoped by
  accident. The rule `GpuStage::cpu()` and `accelerate_prefix_stage` already follow: **fail toward
  the whole-buffer path**, where a wrong answer costs milliseconds and never pixels.
- ⚠ A defaulted trait method rather than a rebuilt stage: `PipelineStage` already carries four of
  them (`accelerate_inner`, `sub_stage_names`, `set_coverage_rasterizer`,
  `has_coverage_rasterizer`), so this is a precedent applied rather than a new shape.

`OutRect { x0, y0, w, h }` is new and lives in `pipeline` beside `Region`. ⚠ It is **not** `Region`:
that type carries an origin plus the *frame's* size and takes its own width and height from the
buffer, which is exactly what a stage producing a sub-rect cannot do — the trap
`render_cached`'s `clip` comment already records.

---

## 4. The gate is derived, never a flag

Engage when **exactly the last prefix step needs re-running** and that step accepts a region:

```
start == steps.len() - 1   &&   apply_region(..) returns Some
```

`render_cached` already knows both. **No drag signal, no new plumbing, and nothing that can
disagree with anything else** — the rule `Session::geometry_mode` was deleted for and
`render_max_dim` was fixed for, both of which were two facts that could disagree.

⚠ **Only the LAST step can be region-scoped, and that is structural rather than a v1 limit.**
Chaining two of them is impossible: step *n−1*'s region-scoped output cannot feed step *n*, because
step *n* is a gather that reads **anywhere** in its source. Sound region chaining needs pre-image
analysis per stage, which is the whole thing this design avoids.

⚠ At the fitted rung the region is `None`, so this never engages there. It engages exactly at
native rungs, which is where the 131.7 ms is.

⚠ The gate also fires when the final checkpoint has been **evicted** while its predecessor
survives. That is fine and arguably better: such a render costs ~6 ms of geometry rather than
131.7, and it recurs only while the eviction stands.

---

## 5. ⚠⚠ The frame dimensions must come from the PREDICTION, never from the buffer

**This is the sharpest hazard in the design.** `render_cached` reads `(pw, ph)` off the prefix
output and hands it to two consumers:

- `CoverageSpec { fw: pw, fh: ph, .. }` — every brush mask's coverage is rasterised at that size.
- the tail's `Region { full_w: pw, full_h: ph, .. }`.

With a region-scoped prefix that buffer **is** the region, so taking the dims off it would
rasterise every mask at the region's size — **a wrong mask, and a coverage cache that thrashes on
every pan**, which the comment at that very line already warns about for a different reason.

So a second defaulted method:

```rust
fn output_dims(&self, w: usize, h: usize) -> (usize, usize) { (w, h) }
```

⚠ The identity default is correct for every stage in the tree today — lens correction, perspective,
spot removal, noise reduction, dehaze and the masked capture corrections all preserve dimensions,
and only `GeometryStage` does not. ⚠⚠ But **a default that is silently wrong for a future
dim-changing stage is a trap**, so a *total* test asserts, for every prefix stage a real `Edits`
can produce, that `output_dims` agrees with what `apply` actually returns.

---

## 6. The region-scoped output is never cached

Structural: that branch does not call `cache.insert`. **A region-scoped checkpoint served for a
different region is the one failure here that would show as a plausible photograph rather than as
an error** — the same class as the vignette's re-centred falloff and the mask that lags a frame.
Guarded by a test asserting the final checkpoint is absent after such a render.

⚠ Consequence, priced rather than hidden: until some later whole-buffer render rebuilds the final,
a **tail** slider at a native rung pays the ~6 ms region-scoped geometry on top of its 19.8 ms
tail. Against 131.7 ms today, and only at native rungs.

---

## 7. Acceptance

**Bit-identity, not a tolerance.** `apply_region(src, rect)` must equal `apply(src.clone())` cropped
to `rect`, **byte for byte** — the same gather, the same operations, the same order.

⚠⚠ **The rotation's centre is `(w−1)/2` of the FULL output, never the sub-rect's.** Using the
sub-rect's centre shifts the picture, and the result is a plausible photograph of the wrong part of
the frame. The same holds for perspective's `cx`, `cy` and `aspect`. That is the one way to get
this wrong and it is what the bit-identity test exists to catch.

- **63 goldens byte-identical.** They never reach this path — the CLI and export pass no region —
  so what they prove is that the two new defaulted methods changed nothing that already worked.
- **The test count moves by exactly what is added**, asserted rather than "still green".
- The drag row in `probe_resamplers` is re-measured at the end; a projection is not a result, and
  this crate has had three composed projections miss.

---

## 8. Accepted caveats

1. **A photo with dehaze or noise reduction active gets nothing.** Those steps follow geometry, so
   geometry is no longer last. ⚠ NR could be admitted later behind a margin — its reach is bounded
   — on this same seam. ⚠⚠ **Dehaze can never be**: its airlight is estimated from the whole frame,
   so a region-scoped dehaze would change the photograph as you pan, which is the exact hazard that
   put dehaze in the prefix.
2. ⚠ **A Perspective drag qualifies only when geometry is inactive**, since geometry follows it and
   only the last step can be region-scoped. Included anyway because it is the same fifteen lines,
   but the honest note is that most photographs carrying a perspective correction also carry a
   crop.
3. ⚠ **A photo turned 90 degrees or mirrored gets nothing**: `orient` runs whole-buffer inside
   `GeometryStage::apply`, so that case declines. Uncommon — a camera's portrait orientation is
   applied at decode through `sizes.flip`, not through `Edits.orientation`. Folding the permutation
   into the rotation's sample coordinates is the extension.
4. **Tail sliders after a straighten pay ~6 ms extra** at a native rung until a whole-buffer render
   rebuilds the final checkpoint.
5. **Nothing at the fitted rung**, where the region is `None`. A fitted drag stays at 40.6 ms, and
   `drag_quality: Faster` remains the lever there.
6. **Nothing for a lens-correction drag**, which is the 1002.1 ms row and remains item 3's
   territory — it is the *first* prefix step, so everything downstream of it re-runs too.
7. One machine, one photograph, and the bench machine was not quiet.
