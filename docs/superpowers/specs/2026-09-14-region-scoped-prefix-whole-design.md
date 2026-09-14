# M8.7 — region-scope the whole prefix from the dragged step onward

**Date:** 2026-09-14
**Status:** design, approved in brainstorming
**Supersedes:** M8.6's gate (`start + 1 == steps.len()`). M8.6's *seam* is kept and generalised.

## 1. The problem, measured

The prefix runs whole-buffer while the tail renders a region. During a drag the dragged step is
invalidated every frame by construction, so its whole-buffer output is computed and thrown away.

Measured in the running app at rung 6246 (`RUSTROOM_DEBUG_RENDER`, 2026-09-14), after the
prefix-cache eviction fix:

```
prefix: ["lens_correction", "geometry", "noise_reduction", "dehaze"]  resuming at 1 of 4
  [1]         geometry    206.2 ms  -> 6246x4170 (26.0 Mpx)
  [2]  noise_reduction    308.4 ms  -> 6246x4170 (26.0 Mpx)
  [3]           dehaze    359.8 ms  -> 6246x4170 (26.0 Mpx)
render:  891.8 ms  rung 6246  frame 1768x1241 (2.2 Mpx)  prefix_ran 3
```

**668.2 of 874.4 ms of prefix work — 76% — is two LOCAL operators re-running at 26 Mpx to display
2.2.** ⚠ The five-step Perspective chain gives 63%; the shorter the prefix, the larger the two
locals' share. There is no chain on which they are not the answer.

⚠⚠ **This does NOT replace the prefix cache, it supplies the half the cache never had.** The cache
answers *"parameters unchanged"* — panning and every tail slider, observed at `prefix_ran 0` and
18.3 ms. It has no answer for *"parameters changing"*, because the dragged step's key moves every
frame. M8.7 answers the second and keeps the first.

## 2. What M8.6 got wrong, and the correction

M8.6's argument — *"step n-1's region-scoped output cannot feed step n, because step n is a
gather"* — is true of **two gathers in a row**. Its conclusion, that only the last step can ever be
scoped, does not follow.

⚠⚠ **But the retraction's own framing is also incomplete.** It says gathers all precede locals, which
is true of `build_prefix_steps`' order — `lens_correction -> perspective -> spot_removal -> geometry
-> noise_reduction -> dehaze -> masked_captures`. What it does not draw out: scoping from
`perspective` puts **`geometry`, a gather, in the middle**, which is exactly the shape it calls
forbidden. That is not an edge case — it is the reporter's own Perspective drag.

The answer is to make a gather in the middle *representable*: a gather can be region-scoped when it
can say what input rect it needs, which for a rotation or a homography is the bounding box of the
back-projected corners plus the resampler's tap radius — analytic and exact.

## 3. The margin, corrected

⚠⚠ **CLAUDE.md's recorded ~110 px is too small by ~1.6x, and the rule it missed is already written
down in this repo.** `local_contrast::kernel_reach` records that **a guided base reaches `2r`** —
every window statistic is a box blur of radius `r` and the coefficients are blurred again.

| stage | reach, full-resolution px |
|---|---|
| dehaze: `2 x REFINE_RADIUS_FULL` (120) + `PATCH_RADIUS_FULL` (20) | **140** |
| noise reduction: max(`2 x LUM_RADIUS_FULL` = 12, chroma 8..32) | **32** |
| **chained (dehaze runs on NR's output)** | **172** |

An undersized margin is a plausible-but-wrong band at the region edge — the worst failure shape.

The economics survive: on a 1768x1241 region, margin 172 gives **3.35 Mpx against 26.0**, a **7.8x**
reduction (110 would have given 2.90 Mpx / 9.0x).

## 4. The seam — one new trait method

```rust
fn input_rect_for(&self, out: Rect, in_w: usize, in_h: usize) -> Option<Rect> { None }
```

*"To produce this rect of my output, I need this much of my input."*

⚠⚠ **`None` is the default, so every stage declines BY CONSTRUCTION** rather than being name-checked
out by the caller. The rule `GpuStage::cpu()`, `accelerate_prefix_stage` and `apply_region` already
follow: **fail toward the whole-buffer path**, where a wrong answer costs milliseconds and never
pixels.

| stage | rule |
|---|---|
| `NoiseReductionStage` | grow by a new `kernel_reach` — max(2 x 6, chroma 8..32) |
| `DehazeStage` | grow by a new `kernel_reach` — 2 x 60 + 20 |
| `GeometryStage` | ⚠ **declines on a non-identity orientation**, matching `apply_region`; otherwise offsets by the crop origin and back-projects four corners through the inverse rotation, bbox, + tap radius |
| `PerspectiveStage` | back-project through the inverse homography, bbox, + tap radius |
| everything else | inherits `None` |

⚠⚠ **Corrected while reading the code, 2026-09-14.** An earlier draft of this table said geometry should back-project through the orientation too. It must not: `GeometryStage::apply_region` already **declines** on a non-identity orientation, because `orient` is a whole-buffer index permutation feeding the rotation's gather. `input_rect_for` must decline on exactly the same condition, or the plan would promise an input rect for an output the forward half then refuses to produce.

⚠ Both back-projections are **exact rather than conservative**, and for a reason worth stating: an affine map and a homography both send lines to lines, so the image of a rectangle is a parallelogram or a quadrilateral and the bounding box of its four corners contains it exactly. ⚠ The homography's exception is a rect crossing the horizon, where the denominator vanishes and the image is unbounded — guarded by testing the mapped corners for `is_finite` and returning `None`, which fails toward the whole-buffer path.

Both `kernel_reach` functions follow `local_contrast::kernel_reach`'s shape **and its 2r rule**, and
carry the same warning that a renderer cropping before the stage must keep at least this margin.

### `apply_region` generalises

Today it assumes `src` is the whole input, true only for the *first* scoped step. Chained, each step
reads the previous step's region output, so the source's origin and full logical dimensions must
travel with it:

```rust
fn apply_region(&self, src: &RegionSrc<'_>, rect: Rect) -> Option<ImageBuf>
```

with `RegionSrc { buf: &ImageBuf, origin: (usize, usize), full: (usize, usize) }`. M8.6's two impls
are the `origin == (0, 0)` case and generalise rather than being rewritten.

### Rect consolidation, as part of this work

⚠ `pipeline::OutRect { x0, y0, w, h }` and `app::RegionRect { x, y, w, h }` are **structurally
identical with different field names**, and this milestone would otherwise add a third. They become
**one type, `pipeline::Rect { x, y, w, h }`**, where the stages live. ⚠ The name is deliberately
neutral rather than `OutRect`: `input_rect_for` returns an *input* rect and `apply_region` takes an
*output* one, and a type called `OutRect` appearing in both positions reads as a bug every time
somebody new looks at it. `pipeline::Region { full_w, full_h, x0, y0 }` is a
genuinely different concept — origin plus full dims, for position-dependent stages like the
vignette — and stays.

## 5. The plan — a pure function, in `pipeline`

```rust
pipeline::region_plan(steps, start, visible, in_w, in_h) -> Option<RegionPlan>
```

Walks **backwards** from the visible rect through `steps[start..n]`, accumulating each step's
required input rect. **Any `None` abandons the whole plan.** Returns the per-step output rects and
the composed frame dimensions.

⚠⚠ **A declining step anywhere at or after `start` kills the entire scoped run**, because a
whole-buffer step needs a whole-buffer input. Two consequences on real chains:

- **spots present + a Perspective drag** -> the run can only begin at `geometry`. Contained.
- **any masked NR or dehaze** -> those steps are last, so **nothing at all can be scoped**.

⚠ It lives in `pipeline` and takes plain rects, so the whole of the hard reasoning — margins,
back-projection, composition, the abandon rule — is unit-testable **with no render engine and no
window**. That is this repo's own placement test: *can this be tested without opening a window?*

## 6. Dehaze's airlight

The airlight becomes a **parameter**, computed from a fixed **256 px replay of the prefix up to
dehaze**, run on a cached 256 px copy of the session's source buffer.

⚠⚠ **It replays from the START OF THE PREFIX, never from `start`.** Replaying from the resume point
would make the airlight depend on *which slider you dragged* — two facts that can disagree, the
shape `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were each deleted for.
Starting from the source buffer makes it a deterministic function of the edits alone: independent of
the region, of `start`, and of the render scale.

⚠ **The 256 px copy is of the engine's own source `Arc<ImageBuf>`**, so it is invalidated when that
buffer is replaced — which is exactly the full-resolution handover, and nothing else. ⚠ The replay
builds its steps through `build_prefix_steps(edits, replay_scale, ..)` at the 256 px scale, so the
radii it uses are the ones that scale demands rather than the render's.

⚠ The estimator already downscales to a fixed 256 px box (`AIRLIGHT_SAMPLE`) — M8.3o did that
deliberately so the estimate cannot vary with render scale — which is what makes this affordable.
The 256 px source copy is made once per session; the replay is a 65k-pixel run.

**Used in BOTH paths, so scoped and whole-buffer agree by construction rather than approximately.**
That is the whole argument for it: the alternatives leave a release-frame pop whose size nobody
knows, and this project's record is that "probably invisible" differences are reported within the
hour.

**Price, stated plainly: this changes the airlight slightly for everyone, exports included**, because
downscale-then-process != process-then-downscale. **The three dehaze goldens move** (`dehaze_plus`,
`dehaze_minus`, `mask_dehaze`), re-authored deliberately with the clipping and distinct-value checks
the repo applies. ⚠ A **Dehaze drag's value is unchanged**, because there the replay reduces to
exactly today's computation.

## 7. The gate — derived, never a flag

Engage when a region is present, `region_plan` returns `Some`, and the margined area is under a
threshold of the frame. ⚠⚠ **No drag signal is plumbed and nothing can disagree** — M8.6's rule kept.
A consequence worth having: it therefore also speeds the **cold entry into 1:1** (observed at
2501.4 ms), not only drags.

⚠ The area threshold will be **swept, not chosen**. `MIN_GPU_BOX_PX` was low by ~8x and
`MAX_TILE_DABS` was sized by a *limit* rather than the data and measured 5x slower than the CPU it
replaced. A constant nobody can re-measure is a constant nobody can defend.

## 8. Caching, release and frame dimensions

- **Scoped outputs are never cached.** M8.6's rule, kept for M8.6's reason: a region-scoped
  checkpoint served for a *different* region is the one failure here that shows as a plausible
  photograph rather than an error. Leaving the insert out makes it unrepresentable rather than
  guarded.
- **On settle: render scoped, then enqueue a whole-buffer render behind it.** The settled picture is
  instant and the cache warms a moment later; the swap is **invisible by construction**, because
  section 6 makes the two paths bit-identical. A gesture begun meanwhile supersedes it — the
  "abandon, don't abort" rule `DecodeWorker` and `batch_export` already follow.
- **Frame dimensions come from composing `output_dims` across the scoped steps, read BEFORE they
  run.** ⚠⚠ M8.6's sharpest hazard, generalised: `(pw, ph)` feeds `CoverageSpec { fw, fh }` and the
  tail's `Region { full_w, full_h }`, so taking them off a region-scoped buffer would rasterise every
  brush mask at the region's size and thrash the coverage cache on every pan.

## 8b. Two plans, not one

This is large enough to split the way M3.1a, the boolean masks and the GPU brush coverage each were,
and for the same reason: a wiring bug must never be confusable with a model bug.

- **Plan 1, headless, entirely in `pipeline`:** the rect consolidation, `input_rect_for` with its four
  impls and two new `kernel_reach` functions, `apply_region`'s generalisation, `region_plan`, and the
  airlight parameterisation. Every hard property — margins, back-projection, the abandon rule — is
  testable here with no engine and no window. **The three goldens move in this plan.**
- **Plan 2, wiring, in `app`:** `prefix_cache.rs` split out first as a pure move with unchanged tests,
  then the gate in `render_cached`, the composed frame dimensions, the warm-behind request, the
  byte-for-byte acceptance test and the measurements.

⚠ Plan 1 ships **inert**: nothing calls `region_plan` until plan 2 exists. That is deliberate and is
M3.1a's precedent — but it is also the W5 failure mode (creative LUTs shipped fully tested and 100%
dead), so plan 2's carrier test is what closes it and must not be deferred.

## 9. Acceptance

- `a_region_scoped_render_matches_the_whole_buffer_path_byte_for_byte`, generalised to a **multi-step
  chain**. ⚠⚠ **With a vignette in the fixture** — the only tail stage that can see wrong frame
  dimensions, and the reason M8.6's version caught a sabotage all 63 goldens were blind to.
- **63 goldens run; 60 byte-identical, 3 dehaze references re-authored** and measured.
- `region_plan`'s abandon rule tested directly: spots present, masked captures present, each blocking
  as designed.
- ⚠ The margin is tested by **sabotage**: an undersized margin must fail the byte-for-byte test at the
  region's edge, or the test is not measuring the thing it is named for.

## 10. Measured, not asserted

1. **The airlight shift in 8-bit levels** on a real photograph, **before** anything is built on it. If
   it moves a picture visibly, that changes the design rather than being absorbed.
2. The area threshold, swept.
3. The real drag before and after, through `RUSTROOM_DEBUG_RENDER`. ⚠⚠ **Not a probe row.** M8.6's
   retraction exists because `probe_resamplers` priced a *stage* against a resume the app could not
   perform, and this file's own rule — *a synthetic bench prices a stage, not a session* — was
   already written down when it happened.

**Projected and explicitly unmeasured:** a Straighten drag **891.8 -> ~113 ms**, a Perspective drag
**~1030 -> ~150 ms**.

## 11. Accepted caveats

1. **A photo carrying a masked NR or dehaze gets nothing.** Those steps are last, so one declining
   step switches the feature off for that photo. ⚠ Re-scanned 2026-09-14 rather than quoted:
   **40 sidecars, 1 carries any mask, 0 carry a masked capture correction.** Including them is three
   problems — region-scoped sub-stages, a per-mask airlight, and a frame-size coverage rasterisation
   (24-64 ms measured) paid every frame once the step stops being cached, which could exceed the win.
   **Gated on that measurement, not on effort.**
2. **Spots block a Perspective drag from scoping above `geometry`.** `spot_removal` is anchored to
   `GeometryStage`'s input so a patch survives a crop, which is worth more than the scoping cost.
3. **Nothing for a lens-correction drag**, the ~856 ms row — it re-runs everything below it. That is
   item 3 of the agreed order and a GPU port, not a restructure.
4. **The machine still spends ~874 ms of CPU per gesture warming behind**, so a rapid series of drags
   keeps the worker saturated and the cache often cold. The answer if that bites is to scope more of
   the prefix, not to cache more.
5. **Three goldens move**, and the airlight changes slightly for every export.
6. ⚠ **The cache budget model is strained and is deliberately NOT addressed here.** At rung 6246 a
   checkpoint is **298.1 MiB and `MID_BUDGET_BYTES` (512 MB) holds exactly one** — the two-tier
   design was sized when a checkpoint was 74.5 MiB and six fit. M8.7 *reduces* the dependence on it,
   so it should be re-measured afterwards rather than guessed at now.
