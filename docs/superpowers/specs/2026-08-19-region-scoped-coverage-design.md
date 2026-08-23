# Region-scoped brush coverage

**Status:** implemented, 2026-08-23. See "Outcome" at the end.

## Scope

Rasterize a brush mask's coverage over the **rendered region** rather than the whole frame, so
painting at 1:1 costs ~1.6 ms instead of ~24 ms — and, if the measurement supports it, so the
pixel effect can render *live* during a drag instead of only on release.

Recorded as the recommended follow-up in M3.1c-B's outcome and chosen there over incremental
splatting: less machinery, and it helps **every** render at zoom rather than only the in-progress
stroke.

**Not in scope:** the tint, which already caps its own rasterization at 1400 px and is unaffected;
and incremental splatting, which this is the alternative to.

## The reversal, stated plainly

⚠⚠ **The region goes INTO the brush cache key, reversing a decision documented emphatically in
both M3.1c-B's spec and CLAUDE.md**: *"the region is not in the key, exactly as `PrefixCache`'s is
not — rasterize whole-frame and sample the sub-rect, so panning never invalidates."*

That rule is correct for the **prefix**. The prefix runs whole-buffer by construction, its output
*is* the frame, and keeping the region out of its key is what makes panning re-run only the tail.

**It was copied to the brush without being re-derived, and there it is the wrong trade.** What it
buys is a brush cache that survives a pan — but panning already re-runs the entire tail, so that
saving is invisible. What it costs is rasterizing the full frame on every paint frame: 23.76 ms at
6246×4170, measured, and reported in use as "very sluggish at 1:1".

The brush is the opposite case and takes the opposite decision.

⚠ **`PrefixCache` keeps its rule; `BrushCache` gets the other one.** The two caches now differ on
a point that looks like an inconsistency, so both sites must say why — otherwise someone will
"fix" one to match the other and silently restore the 24 ms.

## The change

`pipeline::brush::rasterize` gains an optional clip rectangle in **frame pixels**:

```rust
/// Rasterize only what falls inside `clip` — the rendered region, in frame pixels.
///
/// `None` rasterizes the whole frame, which is what the CLI, export and the fit view do.
pub fn rasterize_in(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
) -> CoverageMap;
```

with the existing three-argument `rasterize` delegating `None` — the `Region` precedent again, so
the CLI, export and every golden compile untouched.

Three consequences, in the order they matter:

1. **⚠ `FrameMap` is untouched.** It still maps source → *frame* coordinates. Only the output box
   is clipped, so dab positions are computed exactly as before and nothing about the pre-geometry
   anchoring guarantee moves. This is the property that makes the change safe.
2. **Dabs whose extent falls entirely outside the clip are skipped before any allocation.** That is
   where the saving comes from on a long stroke that runs off-screen — the common case at 1:1.
3. **The `cov` and `layer` buffers shrink** from the stroke's bounding box to *bbox ∩ clip*. At
   full resolution that is the difference between several megapixels of `f32` and about 1.3.

With no region — the fit view, the CLI, every export — the clip is `None` and the output is
byte-identical. The 38 goldens are the check.

## What does not change

- **`outside` and `is_inert`.** An inverted mask still reads `outside` beyond its box, clipped or
  not, so the bug where inversion stopped at the bbox cannot come back through this door.
- **The tint.** It caps its own rasterization at 1400 px on the long edge and is drawn from the
  canvas, not the worker.
- **`MaskStage`'s sampling.** It already indexes by frame pixel (`cov.at(x0 + x, y0 + y)`), which
  is exactly right for a clipped map — outside the clip it reads `outside`, and the region never
  asks for a pixel outside itself.

## Who supplies the clip

Two callers rasterize, and **both must pass the same clip** or the cached map and the fallback
disagree:

- **`MaskStage::apply`**, on the uncached path, passes its own `self.region` as
  `(r.x0, r.y0, w, h)` — where `w`/`h` are the incoming image's dimensions, which *are* the
  region's size. With `region == None` it passes `None`.
- **`BrushCache::get_or_build`** gains the region alongside the frame dimensions, passes it as the
  clip, and includes it in the key.

⚠ **The two must agree, and the type system will not enforce it.** A prebuilt map clipped to one
region handed to a `MaskStage` rendering another would silently sample `outside` across the
mismatch — a mask that fades out mid-image rather than an error. The key includes the region
precisely so this cannot happen, which is a second reason it belongs there beyond invalidation.

## Cache capacity

`CAP = 2` stays.

⚠ With the region in the key a pan now misses — but a miss costs ~1.6 ms, and `REGION_MARGIN`
(192 px) means small pans do not change the region at all, so most panning still hits. The cache's
remaining job is unchanged and is the one that matters: an unrelated slider drag with brush masks
present must not re-rasterize on every frame.

## Live rendering, gated on a measurement

The payoff is that a paint frame at 1:1 becomes roughly 1.6 ms of rasterization plus ~7 ms of
region tail and ~3 ms of pack — affordable to run on every drag frame, which is what Lightroom
does and what Rustroom currently does not.

⚠ **The `drag_stopped`-only rule is not lifted on an estimate.** M3.1c-B measured a real frame
before deciding and then discovered the threshold was measuring the wrong thing; the same
discipline applies here, with more suspicion. The plan measures an actual paint frame at 1:1 and
only then flips the rule.

- **Under ~16 ms** → render live during the drag. That is a 60 Hz budget, and the point of the
  whole change.
- **16–30 ms** → render live anyway, but say so in the docs as a known-marginal path with the
  number recorded.
- **Over ~30 ms** → keep tint-only feedback. The change still made every zoomed render cheaper,
  which is worth having on its own.

⚠ If the rule is lifted, `drag_started`'s render **stays suppressed**. Rendering a one-point stroke
you are about to extend was pure waste and caused the "initial dot" bug; live rendering means
rendering on *motion*, not on press.

## Testing

⚠ **Break the thing each test names, confirm the sabotage compiled, and watch it fail.** Eleven
recorded cases in this project of a test that could not detect its subject, and one sabotage that
failed to compile while looking exactly like a pass.

| Test | Sabotage |
|---|---|
| **`a_clipped_raster_matches_the_whole_frame_one`** | offset the clip origin by one |
| `a_dab_entirely_outside_the_clip_contributes_nothing` | drop the skip |
| `the_clip_does_not_shift_dab_positions` | subtract the clip origin from the dab centre |
| `an_inverted_mask_still_covers_the_clipped_area` | zero `outside` when clipping |
| `no_clip_is_byte_identical_to_the_old_behaviour` | make `None` clip to the bbox |
| `the_brush_cache_misses_when_the_region_moves` | drop the region from the key |
| `the_brush_cache_still_hits_when_only_a_slider_moves` | add the whole `Edits` to the key |

⚠ **`a_clipped_raster_matches_the_whole_frame_one` is the load-bearing test**, and it must compare
*coverage values at frame coordinates*, not the maps' fields. The boxes legitimately differ — that
is the entire point of the change — so asserting on `x0`/`w` would fail against a correct
implementation, the same trap `a_stroke_survives_a_crop` had to avoid in 3.1c-A. Sample every frame
pixel inside the clip through `at()` and require exact equality.

⚠ **`the_clip_does_not_shift_dab_positions` is the one that catches the likely bug.** Clipping
invites subtracting the clip origin somewhere it should not be subtracted, which lands every stroke
off by the region offset — visible only when zoomed, and easy to mistake for a mask that "drifts".

### Canvas coverage

⚠ M3.1c-B produced six bugs, every one in canvas wiring and none caught by a test. If the live
rendering rule is lifted, that lives in `draw_mask_overlay`, so it gets a **headless `RawInput`
test** driving a drag and asserting a render is requested on motion and *not* on press. The seam
exists (`tool_drag_for`, `pan_delta_for`) and this is the first canvas change since that finding —
using it is the point.

## Accepted caveats

1. **Panning invalidates the brush cache**, where it previously did not. Justified above: a miss is
   ~1.6 ms and `REGION_MARGIN` absorbs small pans.
2. **Two caches with opposite rules about the region.** Documented at both sites; genuinely the
   right answer for each.
3. **No incremental splatting.** A single very large stroke at 1:1 still re-rasterizes its visible
   part every frame. That is bounded by the viewport now, which is the fix.
4. **The tint is still a second rasterization**, unchanged from M3.1c-B and deliberate.

## Outcome

**Implemented 2026-08-23.** 664 tests, **no golden moved** — which is the whole safety argument,
since with no region the behaviour is byte-identical.

### The measurement, and where the spec was wrong

Measured on an X-T4 at 1:1 on a full-resolution buffer, best of 5:

| | |
|---|---|
| brush rasterize, full 6246×4170 | 24.64 ms |
| **brush rasterize clipped to 1400×900** | **9.03 ms** |
| region tail 1400×900 | 7.20 ms |
| pack, ~1400×900 | ~0.4 ms |

**A paint frame is ~16.6 ms**, which falls in the middle band the spec fixed in advance: render
live, and record it as known-marginal against a 60 Hz budget rather than claim it is comfortable.
Confirmed smooth in use.

⚠⚠ **The spec predicted ~1.6 ms for the clipped rasterization and that was wrong by 5×.** The clip
bounds the **box**, but every dab that falls *inside* it still costs its full area — and at 1:1 a
dab of radius 0.02 is 125 px across. The bad estimate came from extrapolating the fit-resolution
number (1.76 ms at 1600×1068), where dabs are small in absolute pixels. **A 2.7× win, not 15×.**

That matters beyond the arithmetic: it means the remaining cost is *dab area*, not frame area. If
this ever needs to be faster again, incremental splatting is the lever — and now for a
well-understood 9 ms rather than the 24 ms it had been misattributed to.

### ⚠ The dab skip is an optimisation, not a correctness guard

The plan pre-flagged that this sabotage might legitimately pass, and it did: replacing the
skip with `|_, _| true` leaves every test green. The bounding box is clamped to the clip
regardless, so an out-of-clip dab contributes nothing to the output either way. What the skip
saves is the box being widened to the stroke's full extent and both `f32` buffers being sized for
it. Both the code comment and
`a_dab_entirely_outside_the_clip_contributes_nothing` now say the test guards the *output*, not
the branch.

### The reversal held up

`BrushCache` keys on the region; `PrefixCache` does not. Both sites carry a comment saying why,
because the asymmetry reads as an inconsistency. Panning at 1:1 with a brush mask selected now
misses the brush cache — the trade this change makes — and was not reported as worse.

⚠ The second reason the region belongs in the key proved its worth during implementation: it makes
a clip mismatch unrepresentable. `MaskStage` derives its clip from `Region` plus the incoming
image's dimensions, `render_engine` from `RegionRect`; both must be the same rectangle, and a
mismatch would read `outside` across the difference — a mask fading out partway across the image
rather than an error.

### Nothing else moved

`FrameMap` is untouched, so dab positions and the pre-geometry anchoring guarantee are exactly as
3.1c-A left them. `outside`/`is_inert` are unaffected, so the inverted-mask bug cannot return
through the clip — pinned by `an_inverted_mask_still_covers_the_clipped_area`.
