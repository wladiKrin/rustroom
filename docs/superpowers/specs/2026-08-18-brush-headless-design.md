# M3.1c-A — the brush mask, headless

**Status:** implemented, 2026-08-18. See "Outcome" at the end.

## Scope

The model half of M3.1's third and last mask primitive. Strokes, the rasterizer, the coverage
abstraction and the pipeline seam — **proved against goldens and a real `.RAF` before any canvas
code exists.**

**3.1c-B is the canvas half**: painting gestures, the brush cursor, the tint overlay, panel
controls, and the coverage cache in `render_engine`. **M3.1 closes when 3.1c-B lands**, not here.

⚠ **The split follows 3.1a's own precedent, and for a stronger reason.** CLAUDE.md records why
3.1a was two plans: `pipeline` has zero GUI dependency by rule, the CLI is the permanent test
harness, and *"a canvas bug can never be confused with a model bug."* The rasterizer is the most
intricate thing in M3.1, so proving it before pointer input exists matters more here than it did
there.

## Decisions taken

| Question | Decision |
|---|---|
| Stroke storage | **Vector polylines in `Edits`**, rasterized at render time. |
| Coordinate space | **Pre-geometry source normalised.** Linear and radial keep frame coords. |
| Radius unit | **A fraction of the source image's long edge** — isotropic by construction. |
| Coverage representation | **8-bit, bounded by the painted bbox.** |
| Stroke combination | **`max`**, in order; erase is `min(c, 1 − dab)`. |
| Controls | Size, hardness, erase. **Flow deferred to a later milestone.** |
| Pipeline seam | `MaskStage` rasterizes internally; **the `build_pipeline` family is unchanged.** |

## Why the brush is not a fourth analytic shape

`Mask::coverage(u, v, aspect) -> f32` is a **pure function of position**, and both existing
variants satisfy it. A brush is **sampled data**: answering it would mean walking every stroke and
every point per pixel — hundreds of points over 26 Mpx, billions of distance evaluations on every
slider drag.

So `MaskStage` resolves a mask to a **coverage source** at construction:

```rust
enum CoverageSource {
    /// Linear and radial: a pure function of position.
    Analytic(Mask),
    /// Brush: rasterized, bounded by what was painted.
    Painted(Arc<CoverageMap>),
}
```

and `apply` matches on it **outside** the pixel loop.

⚠ `Mask::coverage` becomes **`Mask::analytic_coverage(..) -> Option<f32>`**, returning `None` for
a brush. A `Mask::Brush => 0.0` arm would compile, run, and silently produce a mask that covers
nothing — the option forces every caller to decide instead. This is the one place in M3.1 where
the enum stops being uniform, and pretending otherwise is what would rot.

## The model

New in `edit-model`:

The `Mask` enum gains its third and final variant:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
    Radial(RadialGradient),
    Brush(Brush),
}
```

⚠ `Copy` is gone from the derive — see below. Externally tagged as before, so every existing
sidecar still reads `{"Linear": {...}}` and loads untouched.

```rust
/// One continuous drag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BrushStroke {
    /// Polyline in **pre-geometry source** normalised coordinates.
    pub points: Vec<[f32; 2]>,
    /// Radius as a fraction of the source image's **long edge**.
    pub radius: f32,
    /// 0 = maximally soft, 100 = a hard edge.
    pub hardness: f32,
    /// Subtract from coverage instead of adding to it.
    pub erase: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Brush {
    pub strokes: Vec<BrushStroke>,
}
```

### ⚠ Radius is a fraction of the LONG EDGE — the aspect trap in its fourth guise

Points are normalised per axis (`x/w`, `y/h`), so that space is anisotropic. A radius expressed as
a fraction of *width* and compared against a fraction of *height* makes every dab an ellipse.
Using the long edge for both axes gives one isotropic measure.

⚠ Note this is resolved by **choosing a unit**, not by applying a correction — unlike
`LinearGradient::coverage` and `mask_tool::hit_test`, and unlike `RadialGradient::coverage`, which
needs no correction at all. Three masks, three different resolutions of the same underlying
problem. `a_dab_is_round_on_a_wide_frame` is the guard.

### ⚠ Pre-geometry coordinates, and why only the brush gets them

`MaskStage` runs in the **tail**, after `GeometryStage`, so `u = (x0+x)/fw` is normalised against
the already-cropped, already-rotated frame. **Change the crop and every mask slides over the image
content.** Invisible for a gradient, tolerable for a radial, ruinous for a stroke painted onto a
subject — crop 10% off the left and the dodge is on the wall instead of the cat.

The asymmetry is principled: **a gradient is compositional** ("darken the top" should stay at the
top after a re-crop) **and a stroke is content-referred** (it must stay on the cat). Existing
sidecars are untouched, which is the second reason not to convert all three.

### ⚠ `Mask` and `MaskedAdjustment` lose `Copy`

A `Vec<BrushStroke>` cannot be `Copy`. Three sites depend on it today —
`stages/mask.rs:35` (`masked: *masked`), and `develop/canvas.rs`'s `let mask = m.mask;` and
`apply_drag(m.mask, ..)`.

⚠ **`mask_tool::apply_drag` should become `&mut Mask` in place**, not `Mask -> Mask`. The
by-value form would clone a whole stroke list on **every frame of every drag**. Its tests change
shape accordingly.

## `CoverageMap`

```rust
/// A rasterized brush mask: 8-bit coverage over a bounding box in **frame** pixels.
pub struct CoverageMap {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// `w * h` samples. Everything outside the box reads 0.
    pub data: Vec<u8>,
}
```

⚠ **Bbox-bounded is not an optimisation bolted on; it is the representation.** A full-frame map at
full resolution is 6246×4170 = **26 MB per brush mask**, and several masks would run to hundreds.
Bounding to the painted region makes memory proportional to what was painted, *and* gives
`MaskStage` the early-out that was deferred at 3.1a and again at 3.1b. One decision closes both.

⚠ **8-bit, not `f32`.** 256 levels across a soft edge is below the visibility threshold, and it is
a quarter of the memory. `an_eight_bit_edge_has_no_visible_banding` measures the worst case —
maximum gain across the softest edge — rather than asserting it by assumption.

## The rasterizer

`pipeline::brush::rasterize(brush, geometry, frame_w, frame_h) -> CoverageMap`

### ⚠ It needs no new pipeline parameter, which is the pleasant surprise

The mapping source-normalised → frame-pixel decomposes into four steps, and **only one of them
needs a dimension we do not already have**:

1. **Orientation** — a normalised permutation, dimensionless.
2. **Rotation** by the straighten angle — needs the pre-geometry **aspect**, not absolute dims.
3. **Crop** — `(u − left) / (right − left)`, normalised.
4. **× frame dims** — which `Region` already carries as `full_w`/`full_h`.

And the pre-geometry aspect derives exactly from the frame dims and the crop rect:
`(frame_w / (right−left)) / (frame_h / (bottom−top))`.

So **`MaskStage::new_in_region` gains a `Geometry` argument** — which
`build_pipeline_split_in_region` already holds in `edits`. `Geometry` is `Copy`, so it is passed
by value.

⚠ **Precisely which signatures move, since "no changes" would be false:** `MaskStage::new` and
`new_in_region` change (both `pub`, but only `build_pipeline_split_in_region` and `pipeline`'s own
tests construct one), and `Mask::coverage` becomes `analytic_coverage`. **What does *not* change is
the `build_pipeline` family** — `build_pipeline`, `build_pipeline_split` and
`build_pipeline_split_in_region` keep their signatures, so the CLI, export and all 35 goldens
compile untouched. That is the property worth protecting, and it is the same one the `Region` work
protected.

⚠ The cache parameter that 3.1c-B needs — an `Option<&BrushCoverage>` on
`build_pipeline_split_in_region`, following the `Region` precedent with delegating wrappers — is
**deliberately not added here**. Adding a parameter nothing yet supplies would be speculative, and
the uncached path has to exist regardless for the CLI.

### Splatting

Rotation preserves circles and the crop does not stretch, so a dab of radius `r` source-long-edge
units splats as a **disc** of `r × pre_long_edge_in_frame_pixels`. Cost is O(dabs × dab area), not
O(pixels × strokes).

Dabs are interpolated along each polyline at **¼-radius spacing**, so a fast drag leaves no gaps.
⚠ A single-point stroke (a click without a drag) must still produce one dab — the interpolation
must not require two points.

### Combination

Strokes are processed **in order**. Within and across strokes:

- paint: `c = max(c, dab)`
- erase: `c = min(c, 1 − dab)`

so erase-after-paint removes and paint-after-erase restores.

⚠ **`max` rather than accumulation is exactly why flow was deferred.** Accumulation is where
stroke-overlap handling gets hard: each dab within one stroke must not re-darken what the previous
dab already covered, which needs per-stroke bookkeeping. `max` needs none, and is correct for a
brush without flow.

### Hardness

Smoothstep from 1 at `d/r = hardness/100` to 0 at `d/r = 1`.

⚠ `hardness` is arithmetically `100 − feather` in `RadialGradient`'s maths. The brush keeps the
brush word because that is what every brush calls it. **Stated explicitly so nobody later
"unifies" the two and inverts one by accident** — which would be silent, since both look
plausible.

⚠ `hardness ≥ 100 − eps` takes a hard-edge branch. The same load-bearing role `FEATHER_EPS`
plays, and the same trap: **the guard is only reachable at exactly `d == r`**, because IEEE
infinity arithmetic gives the right answer on either side. `a_hard_brush_edge_is_not_nan` must
probe the boundary itself — this is the mistake M3.1b's `feather_zero_is_a_hard_edge` made and had
to be corrected for.

## What lands in 3.1c-B

Painting gestures, brush cursor, tint overlay, panel controls, and the coverage cache in
`render_engine`.

⚠ **3.1c-A rasterizes on every render, uncached, and that is deliberate.** The CLI renders once;
the cache exists for interactive editing, which does not exist yet. Its key must not include the
region — rasterize whole-frame, sample the sub-rect — exactly as `PrefixCache`'s does not, so
panning never invalidates. That reasoning belongs with 3.1c-B.

## Testing

⚠ **After writing a test, break the thing it names and watch it fail.** This project has **eight**
recorded cases of a test that could not detect its own subject; M3.1b produced three and the
navigation rework a fourth, that one caused by the *harness* rather than the assertion. A sabotage
that passes is a finding.

### `edit-model`

| Test | Sabotage |
|---|---|
| `a_brush_has_no_analytic_coverage` | return `Some(0.0)` for the brush arm |
| `the_analytic_masks_still_answer` | return `None` for a linear |
| `an_existing_sidecar_without_a_brush_still_loads` | switch `Mask` to `#[serde(untagged)]` — ⚠ **assert the payload, not the variant** |
| `a_brush_round_trips_through_json` | drop a field from the struct |

### `pipeline::brush`

| Test | Sabotage |
|---|---|
| **`a_dab_is_round_on_a_wide_frame`** | scale the radius by width instead of the long edge |
| **`a_stroke_survives_a_crop`** | anchor to frame coords instead of source coords |
| **`a_stroke_survives_a_quarter_turn`** | drop the orientation step |
| `a_stroke_survives_a_straighten_angle` | drop the rotation step |
| `a_single_point_stroke_paints_one_dab` | require two points to interpolate |
| `dabs_do_not_gap_on_a_fast_stroke` | raise spacing to 4× the radius |
| **`a_hard_brush_edge_is_not_nan`** | remove the eps guard — ⚠ **probe exactly `d == r`** |
| `erase_after_paint_removes_coverage` | apply erase before paint |
| `paint_after_erase_restores_coverage` | make erase terminal |
| `the_bbox_bounds_the_painted_region` | return a full-frame box |
| `coverage_outside_the_bbox_reads_zero` | omit the bounds check |
| `an_eight_bit_edge_has_no_visible_banding` | quantize to 4 bits |

⚠ **`a_stroke_survives_a_crop` is the load-bearing test of the whole spec.** Rasterize one stroke
with no crop, then again with a crop, and assert the painted region lands on **the same image
content** both times. The natural wrong version — asserting the bbox is unchanged — would fail
against a *correct* implementation, since the frame moved. Assert content, not coordinates.

### Goldens

35 → 37: `mask_brush_exposure` and `mask_brush_erase`, on the existing chart with hand-written
stroke lists.

⚠ **The existing 35 must come out byte-identical**, and that is the acceptance criterion for the
whole change. ⚠ And per M3.1b's lesson, the exposure lift must be **large enough to be
unambiguous and small enough not to clip** — measure the generated PNG rather than looking at it.

### Real-RAW validation

Render a hand-written sidecar through the CLI on the X-T4 `.RAF` and **measure**, not squint.

⚠ **The RAW case must be cropped and rotated**, because pre-geometry anchoring is the entire claim
of this spec and only a geometry change can falsify it. Render the same stroke with and without a
crop and confirm it lands on the same subject. A stroke on an uncropped photo proves nothing here
— the same shape as the crop-factor bug that was invisible on 400 uncropped files and total on
the one cropped one.

## Accepted caveats

1. **No cache** — every render rasterizes. Correct but slow; 3.1c-B fixes it.
2. **No way to author a stroke** until 3.1c-B, other than hand-editing a sidecar. That is the
   cost of the split, and the same one 3.1a paid.
3. **No flow** — repeated passes do not build up. Deferred to a later milestone; the roadmap file
   in the Obsidian vault needs that note.
4. **Per-pixel adjustments only**, unchanged from 3.1a and unchanged by shape.
5. **No mask intersection or subtraction** between masks; `erase` works only within one brush.
6. **8-bit coverage**, so a very gradual gain over a very soft edge could in principle band.
   Measured, not assumed.
7. **M3.1 does not close here.**

## Outcome

**Implemented 2026-08-18.** 635 tests, up from 607. All 35 pre-existing goldens byte-identical;
`build_pipeline`, `build_pipeline_split` and `build_pipeline_split_in_region` kept their
signatures, so the CLI, export and every golden compiled untouched.

**The central claim validates on a real X-T4 `.RAF`.** The same stroke rendered with and without a
crop lands at source fraction **(0.358, 0.394) in both**, identical to 0.0008, while occupying a
*different frame position* (0.358 vs 0.225 horizontally). Content-referred, not frame-referred —
which is the whole point of pre-geometry anchoring, and only a cropped case could have shown it.

### Where the spec was wrong

⚠ **`HARDNESS_EPS` is a fast path, not a NaN guard.** The spec said it plays "the same
load-bearing role `FEATHER_EPS` plays". It does not. `0.0 / 0.0` in the ramp needs exactly
`q == 1 && hard == 1`, and the `q >= 1.0` early return already excludes `q == 1`; for any `q < 1`
with `hard == 1` the division yields `+inf`, which clamps to the correct `1.0`. Verified by
deleting it and watching every test still pass. `RadialGradient::FEATHER_EPS` genuinely is
load-bearing because that ramp has no equivalent early return — the comment now draws the
contrast, and the test was renamed from `a_hard_brush_edge_is_not_nan` to
`a_hard_brush_has_no_partial_coverage`, which is what it actually pins.

⚠ **`MaskStage`'s `is_empty` early return is an optimisation, not a correctness guard.** The plan
predicted this was possible and the sabotage confirmed it: an empty coverage map reads 0
everywhere, and blending at coverage 0 is bit-exactly the identity. What it saves is a full-image
clone plus a full run of every sub-stage.

### Tests that could not detect what they named

⚠ All found by running the sabotage step; none by review. **Three were genuinely undetecting:**

- **`frame_map_agrees_with_geometry_stage_on_a_straighten_angle`** — a rotation sign error
  displaces a point *in proportion to its distance from the centre*. The original probe sat at
  0.34 of the half-width, where flipping the sign moves it **1.34 px**, inside the suite's 1.5 px
  tolerance. Now 12° at 0.71 of the half-width, moving 5.61 px. **A geometric test needs leverage,
  not merely a plausible input.**
- **`a_dab_is_round_on_a_wide_frame`** — on an 80×40 frame the long edge *is* the width, so
  scaling the radius by width instead of the long edge was a **no-op**. A portrait case, where the
  two differ by 2×, now discriminates.
- **`overlapping_dabs_in_one_stroke_do_not_double_up`** — asserted a range (`> 0 && < 250`), and
  the accumulated value (181) sat inside it. Now compares against a single dab **exactly**, which
  `max` reproduces and accumulation cannot.

**Two more asserted something false about a correct implementation**, a related but distinct fault:

- **`a_soft_dab_ramps_from_the_centre_outward`** demanded exactly 255 at the centre. Normalised
  0.5 of an 80 px frame is `x = 40.0` — a pixel *boundary*, while pixel centres sit at `x + 0.5` —
  so no sample coincides with the dab centre and the peak is 254. The test was wrong, not the code.
- **`an_eight_bit_edge_has_no_visible_banding`** asserted `worst <= 8`, a threshold picked without
  computing it. A smoothstep's steepest gradient is 1.5× its average, so the ramp inherently steps
  ~12/255 over this radius. The bound is now derived from the radius plus one quantization step.

### A process finding

⚠⚠ **A sabotage that fails to compile looks exactly like a sabotage that passes.** Placing
`#[serde(untagged)]` *above* the `#[derive]` rather than below it made the edit-model sabotage a
compile error; grepping the output for a test result found nothing, which reads as a clean pass.
**Confirm a sabotage compiled before believing its result.** Every sabotage in this task was run
through a helper that checks for compile errors first.
