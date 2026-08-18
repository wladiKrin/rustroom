# M3.1c-B — the brush canvas

**Status:** spec, 2026-08-18.

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
