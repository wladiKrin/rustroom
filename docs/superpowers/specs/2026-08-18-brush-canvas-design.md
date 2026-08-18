# M3.1c-B — the brush canvas

**Status:** implemented, 2026-08-18. See "Outcome" at the end. **M3.1 is closed.**

## Scope

The authoring half of M3.1's last mask primitive, and **the change that closes M3.1**. Painting,
the brush cursor, the coverage tint, panel controls, and the coverage cache in `render_engine`.

3.1c-A landed the whole model headless: strokes in `Edits`, the rasterizer, `FrameMap`, and
`MaskStage`'s `CoverageSource`. Strokes are currently authorable only by hand-editing a sidecar.

⚠ **When this lands, the parked button/layout work is unblocked** — masking was the concept the
layout had nowhere to put, which is why that work was deferred until after M3.1 rather than done
around its absence.

## Decisions taken

| Question | Decision |
|---|---|
| Paint vs pan | **No mode.** Left-drag is already free — the navigation rework did that on purpose. |
| Paint cost at 1:1 | **Build simple, measure, then decide** on incremental splatting. |
| Tint source | **The canvas rasterizes its own at screen resolution**, for instant feedback. |
| Cache key | `(strokes, geometry, frame dims)` — ⚠ **never the region**. |
| Tool state | On `RustroomApp`, **not persisted**. |

## The piece of maths that does not exist yet

⚠ **Painting needs the inverse of `FrameMap`.** 3.1c-A built source → frame, because that is the
direction splatting a dab needs. A pointer gives a *screen* position, which maps to a frame pixel,
and the stroke must be recorded in **source** coordinates — the whole basis of the anchoring
guarantee. So the map has to run backwards.

`FrameMap::inverse_point(x, y) -> [f32; 2]` undoes the four forward steps in reverse:

1. frame pixels → normalised, then **un-crop**: `ru = left + (x / fw) * (right − left)`.
2. **un-rotate** by `R(−θ)`, in units of the oriented image's height so the space is isotropic —
   the same requirement the forward step has.
3. **un-permute** the orientation: the inverse of the forward table.
4. **un-mirror**.

And an inverse radius scale, since the size slider is in screen points:
`radius_source = radius_screen / (screen_px_per_frame_px × radius_scale)`.

⚠ **This is the most likely thing in 3.1c-B to be quietly wrong**, for the same reason 3.1c-A's
forward map was: it is a hand-inversion of a composed transform, and an error in it produces
strokes that land *near* the pointer rather than obviously wrong. It gets two independent guards —
a round trip against the forward map, and agreement with `GeometryStage` itself — described under
Testing.

## Painting

There is **no paint mode**, and that is the payoff of doing the navigation rework first. When the
selected mask is a brush, `tool_drag` — already gated to the primary button — paints:

- **`tool_drag_started(&resp)`** → push a new `BrushStroke` carrying the current size, hardness and erase
  flag, with the pointer's source position as its first point.
- while `tool_drag(&resp)` holds → append a point, **decimated** to a minimum spacing.
- **`drag_stopped`** → one `history.push`, so one stroke is one Ctrl+Z. `History<Edits>` is
  snapshot-based, so this is free.

⚠ **Decimation is not tidiness.** A drag at 60 fps emits a point every ~16 ms; a three-second
stroke is ~180 points, most of them a pixel apart, and all of them go into the sidecar as JSON.
Minimum spacing is **¼ of the stroke's radius in source units** — the same figure the rasterizer
uses for dab spacing, so the recorded path is exactly as dense as the splatting needs and no
denser.

⚠ **The live stroke is appended directly into `Edits`**, so it renders as you draw with no
separate preview path. That means the canvas must call `s.request` — the lesson from 3.1a's first
real-use bug, where `draw_mask_overlay` mutated `Edits` and never asked for a frame, so the image
showed the mask's previous position until some later edit happened to request one.

⚠ **A click without a drag paints a single dab.** Accepted: it is one undo step, and it is what
every editor does.

## The tint and the cursor

Both are painted by the canvas, and both are instant.

**The tint** calls `pipeline::brush::rasterize` with the **on-screen rect** as the frame — roughly
1400×900, sub-millisecond for any plausible stroke. Red at 50% alpha, shown while a brush mask is
selected, with a toggle to hide it.

⚠ **The duplication is deliberate and is the point.** The same strokes are rasterized twice, at
different resolutions, by the canvas and by the render worker. The alternative — having the worker
return its coverage map with the frame — has exactly one rasterization and one source of truth,
and was rejected because the tint would then **lag the stroke by a full render**. For a brush,
seeing the mark appear under the cursor *is* the experience; a spongy tint while drawing is worse
than a little duplicated work.

⚠ Red is chosen because it is conventional (Lightroom) and because **none of the four accents is
red** — `Neutral`, `SteelBlue`, `Teal`, `Violet` — so the tint cannot be confused with active
state whatever the user's accent.

**The cursor** is a circle at the pointer at the current screen-space radius. Without it the size
slider is guesswork, because a screen-point size has no other on-screen representation.

Both live in `paint_mask`'s brush arm, which 3.1c-A deliberately left empty with a comment saying
so.

## The cache

`BrushCache` beside `PrefixCache` in the render worker — `render_cached` already takes
`&mut PrefixCache`, so a second `&mut` alongside it needs no new plumbing. Keyed by
`(strokes, geometry, frame dims)`.

⚠ **The region is not in the key**, exactly as `PrefixCache`'s is not: rasterize whole-frame and
sample the sub-rect, so panning never invalidates.

⚠ **It misses on every frame while you paint, and that is correct** — the strokes really did
change, so there is nothing to reuse. Its job is that dragging *Exposure* with a brush mask present
does not re-rasterize the strokes on every frame of that drag.

The pipeline seam is `Option<&BrushCoverage>` on `build_pipeline_split_in_region`, with the
existing wrappers delegating `None` — the `Region` precedent, which gave that work "zero call-site
churn". The CLI, export and all 37 goldens must compile untouched, and the uncached path 3.1c-A
built stays as the fallback.

**Capacity: two entries per brush mask's worth of work — concretely, `CAP = 2`.**

⚠ **Not one, and the reason is the zoom guarantee `PrefixCache` already protects.** Its
`FINALS_CAP` is 2 precisely so that fit and native are both warm and "zooming in and out is instant
rather than recomputing each way". The frame dimensions are in this key too, so a single entry
would be evicted on every zoom change and re-rasterized on the way back — reintroducing at the
brush layer exactly the cost that decision removed at the prefix layer.

⚠ If a photo with several brush masks proves slow, `CAP` is the constant to raise — stated so the
next person does not have to rediscover which knob to turn.

## Measurement, as an explicit step

⚠ **The threshold is stated in advance so the decision is made against a number rather than an
impression.**

Measure with the existing `bench_preview` example: one stroke of ~50 dabs, rasterized at fit
resolution and at 1:1 on the full-resolution buffer.

- **If a paint frame at 1:1 stays under ~30 ms**, ship the simple version. 30 ms is the point at
  which a drag stops feeling attached to the pointer.
- **If it exceeds that**, build incremental splatting: the cache holds the map for committed
  strokes and only the in-progress stroke's new dabs are splatted onto a copy.

⚠ This ordering is not caution for its own sake. The progressive-sharpen feature was built on
reasoning, shipped, debugged three times and removed — while the prefix cache's measurement
*redirected its design* before any code was written. The difference was measuring first.

## Panel and state

An `Add brush` button beside the other two. Size / Hardness / Erase join the shape-properties
block that already holds Feather and Invert, shown only for a brush selection.

Tool state — `brush_size` (screen points), `brush_hardness`, `brush_erase`, `show_mask_tint` — lives
on `RustroomApp`, **not** in `Settings`.

⚠ Not persisted, and the reason is specific: a size in **screen points** is view-dependent, so
restoring last session's value on a different window size or zoom restores a different brush. The
other three could be persisted, but splitting one tool's settings across two lifetimes is worse
than one honest rule.

## Testing

⚠ **After writing a test, break the thing it names and watch it fail — and confirm the sabotage
compiled.** This project has eleven recorded cases of a test that could not detect its own subject,
and one case of a sabotage that failed to compile while looking exactly like a pass.

The three ways those tests were blind, all seen in 3.1c-A, are the checklist for the ones below:
**no leverage**, **a sabotage that is a no-op on the chosen fixture**, and **an assertion band wide
enough to contain the bug**.

### `FrameMap::inverse_point`

| Test | Sabotage |
|---|---|
| **`inverse_point_round_trips_the_forward_map`** | drop the un-mirror step |
| **`inverse_point_agrees_with_geometry_stage`** | flip the un-rotation sign |
| `the_inverse_radius_scale_round_trips` | scale by width instead of the long edge |

⚠ **The round trip must use leverage**: 12° at 0.71 of the half-width, and a **portrait** frame for
the radius case. Those two numbers are not arbitrary — they are precisely the fixtures 3.1c-A's
sabotages proved were needed, after a 7° probe at 0.34 of the half-width and an 80×40 frame both
passed against broken code.

⚠ A round trip alone is **not sufficient**: a forward and inverse that are wrong in mirrored ways
round-trip perfectly. That is why agreement with `GeometryStage` is a separate test rather than a
nicety.

### Painting

| Test | Sabotage |
|---|---|
| `a_drag_appends_exactly_one_stroke` | push a stroke per motion event |
| `a_click_without_a_drag_paints_one_dab` | require two points |
| **`points_are_decimated_to_the_minimum_spacing`** | remove the spacing check |
| `a_settled_stroke_pushes_one_history_entry` | push on every motion event |
| `painting_requests_a_render` | drop the `s.request` call |
| `a_drag_does_not_paint_when_the_selected_mask_is_not_a_brush` | paint regardless of variant |

⚠ `points_are_decimated_to_the_minimum_spacing` must feed **many closely-spaced** motion events and
assert the recorded count is far below the event count. Asserting merely "some points were
recorded" is the wide-band mistake.

### Not tested, and why

The tint's appearance and the cursor circle are egui painting with no logic worth pinning; their
correctness is "it looks right", which is what the GUI check is for. **The plan will say so rather
than padding the suite** — this codebase has explicitly declined to pad tests before, in the
view-demosaic work.

## GUI validation

⚠ This is a feel change and no test can judge it. The GUI check must cover: painting on the
subject; the tint appearing under the cursor without lag; the cursor circle tracking the size
slider; erase removing and re-painting restoring; **one Ctrl+Z per stroke**; and — the one that
matters most — **painting, then changing the crop, and confirming the stroke stays on its subject**.

## Accepted caveats

1. **Strokes are rasterized twice** at different resolutions, by the canvas for the tint and by
   the worker for the render. Deliberate, for responsiveness.
2. **No flow** — repeated passes do not build up. ⚠ Deferred to a later milestone; the roadmap
   file in the Obsidian vault still needs that note.
3. **No auto-mask / edge detection.** A separate feature.
4. **No brush-size keyboard shortcuts** (`[` / `]`). An obvious follow-up.
5. **Tool state is not persisted**, by the reasoning above.
6. **A stray click paints a dab.**
7. **One cache entry**, so several brush masks on one photo re-rasterize.

## Outcome

**Implemented 2026-08-18, and M3.1 closes with it.** 655 tests, 38 goldens, no golden moved by any
canvas change.

### The measurement, and the decision it did *not* settle

| frame | rasterize |
|---|---|
| fit 1600×1068 | 1.62 ms |
| half 3123×2085 | 6.11 ms |
| full 6246×4170 | 23.76 ms |

The spec fixed a ~30 ms threshold in advance to keep the incremental-splatting decision honest.
The number landed almost exactly on it — and then **real use showed the threshold was measuring
the wrong thing entirely.** Two of the three costs at 1:1 turned out to be waste I had introduced,
not inherent work. Fixing them made painting smooth without any incremental machinery.

### ⚠⚠ Six bugs, all found by using it, none by the tests

This is the finding of the whole feature and it should not read as bad luck.

| bug | cause |
|---|---|
| wgpu crash: 711 MB vertex buffer | tint drawn as a mesh — millions of quads across a soft gradient |
| the softest brush still painted at full strength | `hardness` shapes an edge; **nothing controlled strength** |
| invert covered only a rectangle around the strokes | `CoverageMap` had no value for outside its bbox |
| a drawn line rendered as its first dab only | `drag_started` kicked off a full-res render of a one-point stroke, which landed late and painted over the good one |
| sluggish at 1:1 only with the mask shown | the tint rasterized at **`frame_rect`**, which at 1:1 is the *whole frame*, not the viewport |
| the tint drifted with the window when panning | the cache stored a **screen rect**, so a hit survived a pan |

**Every one is in the canvas wiring.** The pure functions — `FrameMap`, `inverse_point`, the
rasterizer, decimation — carry ~30 tests between them and produced not one defect. The suite
covers arithmetic well and interaction hardly at all, and headless `RawInput` tests of the drag
lifecycle would have caught at least the first, fourth and sixth.

⚠ **The last one is a regression of a bug this project had already fixed.** 3.1a's overlay drifted
while panning because it was placed against the wrong rect; the fix made every coordinate
frame-relative and CLAUDE.md records it. Caching a *derived screen position* quietly undid that
guarantee. **Cache the expensive thing, never the placement.**

### Where the spec was wrong

⚠ **"The tint calls `rasterize` with the on-screen rect as the frame — roughly 1400×900,
sub-millisecond."** `frame_rect` is the whole frame's on-screen rect, so at 1:1 on a
full-resolution buffer it is ~6246×4170. The tint was rasterizing at full resolution and uploading
a 26 MB texture. It is capped at 1400 px on the long edge now and stretched by the GPU; the cost
is constant in zoom and a soft edge is invisible under a 50% overlay.

⚠ **"One quad per run of equal coverage" was defended as sufficient and is not.** Run-length
collapses a stroke's flat interior — but across a *soft* brush's gradient almost every pixel
differs from its neighbour, so every run is length 1, and a large soft stroke is mostly gradient.
The optimisation helped precisely where the cost was not. A texture is one quad regardless.

⚠ **Flow should never have been deferred.** The spec inherited "flow is a later milestone" from
3.1c-A. Without it `hardness` was the only brush control that looked like strength and was not
one: the softest brush painted at full strength and repeated passes did nothing. Pulled in, with a
two-level rule — per-stroke `max`, then composite across strokes — that turned out cleaner than
the deferral feared.

### Diagnosis notes worth keeping

⚠ **Instrumentation beat reasoning twice, and both times I reached for it late.** The "first dab
only" bug survived three readings of the drag code because the drag code was *correct* — the
points were all there. One `RUSTROOM_DEBUG_BRUSH` line (`14 strokes, last has 42 points,
max_dim 6246`) reframed it instantly as a stale render. The env-gated print is kept.

⚠ **The user's phrasing was the diagnosis, again.** "Responsive without the mask, sluggish with it
on" isolates the tint from the render exactly, and stopped me building incremental splatting for a
bottleneck that was not there. Symptom sequences are evidence — read them literally.

## Recommended follow-up: region-scoped coverage, not incremental splatting

Painting still defers the pixel effect to release. Lightroom shows it live.

⚠ **The structural difference is resolution scope, and it points somewhere better than incremental
splatting.** Lightroom's Develop view renders at *screen* resolution, so at 1:1 it works on the
visible portion only. Rustroom rasterizes brush coverage **whole-frame**, deliberately, so that
the region stays out of the cache key and panning never invalidates — which makes panning free and
painting expensive.

**Rasterize the coverage over the rendered region instead.** At ~1400×900 that is ~1.6 ms rather
than ~24 ms, cheap enough to render live on every drag frame. Panning would re-rasterize, but
panning already re-runs the entire tail, so the trade is consistent with the rest of the pipeline.

⚠ It is *less* machinery than incremental splatting and strictly more useful: incremental helps
only the in-progress stroke, while region-scoping helps every render at zoom, including a slider
drag with several brush masks present.
