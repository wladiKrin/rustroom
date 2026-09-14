# M8.8 — a region-scoped lens correction

**Date:** 2026-09-15
**Status:** approved

## One sentence

`LensCorrectionStage` implements the `input_rect_for` / `apply_region` pair, so `region_plan`
stops abandoning at step 0 and a cold entry into 1:1 scopes the whole prefix chain.

## Why this and not the GPU port

The agreed systematic order names *"port lens correction to the GPU"* as item 3, and that item is
argued from a real number: after M8.5 lens correction is **~75% of the resampling prefix** and is
**on by default for every RAW**. Both halves are still true. What changed is that M8.7 shipped a
mechanism the GPU port cannot use, and measuring the stage before designing anything showed the
scoping route is worth roughly three times as much on the one case that is actually slow.

M8.7's own retraction states it plainly: a cold entry into 1:1 measures **2392–2528 ms and is
UNSCOPED**, because `lens_correction` is **step 0** and has no `input_rect_for`, so `region_plan`
abandons before it starts. Every downstream step — perspective, geometry, noise reduction, dehaze —
already accepts a region and gets nothing, because one decline anywhere abandons the whole plan.

So the two candidates are not alternatives on the same axis:

| | what it changes | the cold 1:1 entry |
|---|---|---|
| the GPU port | lens correction alone, whole-buffer | ~2400 → ~1900 ms (~1.25× on that step, ~3.5× on the stage) |
| **`input_rect_for`** | **unlocks the whole chain M8.7 already built** | **~2400 → ~210–300 ms** |

The user chose scoping first, then a re-measurement to decide whether the GPU port is still worth
doing. This spec is the first half.

## What is measured, and by what

`crates/app/examples/probe_lens.rs`, written before this spec because `CLAUDE.md`'s own recorded
numbers for this stage do not compose — it holds "400 ms field + 765 ms resampler" from the
per-pixel era beside a 309 ms whole resampling prefix after row batching, and both cannot describe
the same stage.

### Where the time goes

| | 6.5 Mpx (rung 3123) | **26.0 Mpx (1:1)** |
|---|---|---|
| the whole stage | 169.0 ms | **695.8 ms** |
| the displacement **field** | 10.0 ms | **35.6 ms** |
| the **resampling**, by difference | 150.7 ms | **646.8 ms** |

The field is **5–6%** of the stage. That is the number that reframes the milestone twice over: it
says the GPU port would be attacking the 94% (a real target), and it says a scoped render pays
almost nothing for computing the field over a wider row than it uses.

### The margin

```
worst displacement anywhere: 50.59 px
margin = ceil(that) + resampler radius 2 = 53 px
a 1792x1394 region margins out to 1898x1500 — 10.9% of the frame
```

### The scoped cost

`71.2 ms` for a 1792×1394 region (2.5 Mpx) against **695.8 ms** whole-buffer — **9.8×**, on the
stage's own inner loop.

## The design

### 1. The margin cannot be perspective's corner bound, and that is the one real hazard

`PerspectiveStage::input_rect_for` bounds an output rect by mapping its **four corners** and taking
their bounding box, plus the resampler's radius. Its doc comment says why that is exact: *"a
homography sends lines to lines, so the image of a rectangle is a quadrilateral and the bounding
box of its four mapped corners contains it exactly."*

**A radial map bows every edge.** The image of a rectangle under `identity + radial displacement`
is a curved quadrilateral whose extreme can sit at an **edge midpoint**, not at a corner. A corner
bound would therefore under-estimate — and the failure mode is exactly the one the seam warns
about: a **dark band at one edge of the region**, a plausible photograph of nearly the right thing.

> ⚠⚠ **Corrected in place during execution, because that phrasing is too loose and the precise
> condition is sharper.** ptlens is `r_d = r · (b·r² + (1 − b))`, so the radial factor is
> **increasing** in `r` for a **pincushion** correction (`b > 0`) — and then every extreme of a
> rect's image really does sit at a corner, so `PerspectiveStage`'s bound would be sound. It is
> **decreasing** for a **barrel** correction (`b < 0`), and the extreme then sits at an **edge
> midpoint** whenever the rect's span brackets the optical centre. Measured on a 400×300 fixture
> at `b = −0.05`: the corner box misses by **9.2 px**, against a Lanczos3 tap radius of 3.
>
> Two consequences for the tests. The byte-for-byte equivalence test **cannot see this** —
> transplanting the corner bound passes it. And a test using only the pincushion fixture, or only
> rects inside one quadrant, cannot see it either however large the displacement. Hence
> `every_source_a_region_reads_lies_inside_the_rect_it_asked_for`, which sweeps both signs of `b`
> and uses rects that bracket the centre in both axes.

The bound that *is* sound is the **largest displacement anywhere in the frame**: no pixel can move
further than the furthest-moving pixel, whatever shape the rect is. So

```
input_rect_for(out) = out.grown(margin), clipped into the input
margin = ceil(max_displacement) + resampler.radius()
```

It is coarser than a per-rect bound and it is cheap: 53 px on a 1792×1394 region is 10.9% of the
frame, far under `SCOPE_MAX_AREA_PCT`'s 50.

### 2. `Corrector::max_displacement()`

Computed once when the corrector is built, sampled over the frame rather than at four points.
Its guarantee is a **test, not an argument**: `no_pixel_displaces_further_than_the_margin` scans
**every** pixel and all three channels of a real-sized fixture and asserts nothing exceeds it.

Three channels, because TCA gives R, G and B different source points and the margin must bound the
worst of them.

### 3. `apply_region` always computes the field from x = 0

`apply_geometry_distortion` computes each `x` as `(xu + ns * i as f64) as f32` — **from scratch per
`i`, not incrementally**. Starting the field at `x_start = rect.x` re-associates that f64 addition,
and f64 addition is not associative: the discrepancy is ~1e-16 against an f32 spacing of ~1.2e-7,
so about one pixel in 1e9 could round to a different f32. Not a visible bug, and not *provably*
bit-identical either — and "a faint band that appears one pixel in a billion" is precisely the
class of thing this project has paid for.

So the field is always filled from **x = 0** and the loop stops at the rect's right edge.
Bit-identical by construction. It costs ~2% of a region render (the field is 5%, and the wasted
prefix of each row is a fraction of that) against ~16% for filling whole rows blindly — and it is
the same move `rotate` already makes by **being** `rotate_region` at full extent.

### 4. Nothing else changes

- **`output_dims`** — lens correction preserves dimensions, so the trait's identity default is
  already right. No override.
- **`PrefixStepKey`** — untouched. The key is derived from the edits; a region-scoped render is
  never cached, so no key can go stale.
- **The gate** — untouched. `SCOPE_MAX_AREA_PCT` is applied in `render_engine::render_cached` on
  `plan.out_rects.first()`, the **top** of the chain, against `plan.frame`. A longer chain changes
  nothing about how it is read.
- **No `Edits` field, no golden, no GPU code.**

## Tests

1. **`no_pixel_displaces_further_than_the_margin`** — exhaustive over every pixel and all three
   channels of a real-sized fixture. This is what makes the margin a guarantee rather than a
   sampled hope, and it is the only test that can see an under-estimate.
2. **A byte-for-byte region-vs-cropped-whole equivalence** for the stage itself. Lens correction is
   a pure gather, so bit-identity is achievable here (unlike a chain carrying a local, which
   M8.7 bounds to one ulp) — and it is what the compute-from-x=0 rule buys.
3. **A carrier proving a cold chain really scopes at step 0.** Every correctness test in the
   workspace passes with the pair deleted, because a scoped render and a whole-buffer one produce
   the same picture by design. That is the W5 failure mode — creative LUTs shipped fully tested and
   100% non-functional — so the carrier asserts the **plan's length**, not merely that scoping
   happened.

## The projection, stated as a projection

A cold entry into 1:1 at ~2400 ms becomes:

```
lens ~81 + perspective ~15 + M8.7's measured 114–206 for geometry/NR/dehaze
  ≈ 210–300 ms, about 8–12×
```

That is arithmetic over measured parts, not a measurement. **The milestone is judged in the running
app through `RUSTROOM_DEBUG_RENDER`, never by a synthetic bench row** — the rule that has now
corrected two milestones' headlines.

## Caveats, accepted

1. **Nothing for a photo carrying spots.** `spot_removal` sits between perspective and geometry and
   still declines, so that chain cannot scope above `geometry`. It is anchored to `GeometryStage`'s
   input so a patch survives a crop, which is worth more than the scoping.
2. **Nothing for export or the fit rung.** Both are whole-buffer; the region is `None` at fit.
3. **The settle still spends a whole-buffer render in the background.** That is M8.7's accepted
   caveat (4) rather than a new one — the warm-behind is what leaves a checkpoint, and it is real
   CPU spent whether or not anything else happens.
4. **The margin is frame-global, so it is coarser than a per-rect bound.** Measured at 10.9% of the
   frame on the region that matters; it becomes expensive only for a lens whose worst displacement
   is a large fraction of the frame, which no profile in this catalogue is.
5. **One machine, one photograph, one lens profile.**
