# Perspective correction — design

**Date:** 2026-08-31
**Milestone:** M3.2's remaining half. Closes the last outstanding slice of Phase 3 apart from
creative 3-D LUTs.

## What this is

Manual keystone correction: four sliders — **Vertical**, **Horizontal**, **Aspect**, **Scale** —
that build one perspective homography, applied as its own pipeline stage between lens correction
and geometry.

**Guided control points are a later slice, not this one** (user's call, 2026-08-31). The two are
sequenced the way M3.1's three mask primitives were: prove the transform, the edit model, the
cache key and the panel first, with no canvas interaction in existence, so that a canvas bug can
never be confused with a transform bug.

## ⚠⚠ The finding that reframed the feature

`docs/BACKLOG.md` said *"The maths is `mod_pc` in the `lensfun` crate and
`Modifier::enable_perspective_correction` is already available; the work is the UI."*

**That is true only of the guided half.** Reading the vendored `lensfun` 0.7.0 source:

- `enable_perspective_correction(x: &[f32], y: &[f32], d: f32) -> bool` takes **4–8 pixel
  coordinates** lying on lines that should be vertical or horizontal (`modifier.rs:226`). It runs a
  two-sided Jacobi SVD and solves for the camera rotation. There is no "vertical keystone = 30"
  input anywhere in `mod_pc`.
- So **the slider half cannot use `mod_pc` at all.** It needs a homography we build ourselves,
  which is a few dozen lines and no new dependency.

Two further facts, recorded now because they are what the guided slice will need and they are
easy to get wrong later:

- ⚠ **`build_perspective_state` is a public free function** taking *normalised* coordinates
  (`mod_pc.rs:546`), so the guided slice does **not** have to go through `Modifier` and therefore
  does not need a matched lens profile. Only `Modifier::new` requires a `Lens`.
- ⚠ **At 8 control points lensfun estimates the focal length itself** (`mod_pc.rs:341-348`:
  *"Over-determined: prefer the fourth line over the focal length"*). At 4 points it uses the `f`
  it was given. So the guided slice can work on unprofiled lenses and on JPEGs, and the RAW-only
  limitation the other three lens corrections carry need not apply to it.

## The edit model

```rust
/// Manual perspective correction — keystone about both axes, plus the two controls that make it
/// usable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Perspective {
    /// Keystone about the horizontal axis (camera pitch). ±100 → ±30°.
    pub vertical: f32,
    /// Keystone about the vertical axis (camera yaw). ±100 → ±30°.
    pub horizontal: f32,
    /// Counteracts the stretch a keystone correction causes. ±100 → ×1.4 / ÷1.4.
    pub aspect: f32,
    /// How far to scale the corrected image, 0..100. 0 leaves the empty corners; 100 scales
    /// until they are gone.
    pub scale: f32,
}
```

⚠ **Its own field on `Edits`, not a member of `Geometry`.** That is what buys the separate cache
checkpoint: sharing `PrefixStepKey::Geometry` would make every crop drag and every Straighten
frame re-run the perspective resample.

⚠ **`Default` is hand-written, not derived** — `scale` is **100** at neutral, and the
container-level `#[serde(default)]` fills a missing field from it. A derived `Default` would give
every partial sidecar `scale: 0`, silently. `LensCorrection::default` already carries this exact
note, and `RadialGradient::default` and `ColorGrading::default` before it.

⚠ **`is_active()` is `vertical != 0 || horizontal != 0 || aspect != 0`, not `!= default()`.**
`scale` is a *modifier*: dragging it alone must not insert a stage that would only resample the
image for nothing. The rule `NoiseReduction`, `Vignette` and `Sharpening` already follow.

**Edit group: Geometry** (`GEOMETRY_GROUP`, index 6), which defaults **off** for presets, paste
and sync — a keystone framed for one composition is meaningless on another. ⚠ Three exhaustive
constructs make that grouping a compile error until it is decided: `Edits::merge_from`'s
destructure, `pipeline::all_edits`, `edit_model::all_nonneutral`, and `edit_clipboard`'s payload
test.

## The maths

One 3×3 homography, built once at stage construction, inverted once, then inverse-mapped per
output pixel with bilinear sampling — the shape `rotate_bilinear` already has.

Work in normalised coordinates centred on the frame, with **half-height = 1**. A point `(x, y)`
is the ray `(x, y, f)`; rotating the camera and reprojecting gives

```
H = K' · R · K,    K = diag(1, 1, f),   K' = diag(f, f, 1)
R = R_pitch(θv) · R_yaw(θh)
```

then Aspect as a separate `diag(s, 1/s)` with `s = 1.4^(aspect/100)`, and Scale as `diag(k, k)`.

### ⚠⚠ The assumed focal length is a FIXED CONSTANT, not read from EXIF

`F_NORM = 2.0`, i.e. about a 24 mm lens on full frame. This looks like a shortcut and is not.

For **any** assumed `f` there is a `θ` that sends the vertical vanishing point to infinity —
`tan θ = −f / y_v` — so the verticals come out **exactly parallel whichever value is chosen.**
What differs is how much the rest of the frame stretches, and **that is precisely what the Aspect
slider corrects.** So a fixed constant costs nothing that is not recoverable with the control
sitting next to it.

Against that: reading `Capture.focal_len` would make the tool behave one way on a RAW and a
different way on a JPEG carrying no EXIF — two behaviours that differ invisibly, which is the
failure this project has paid for repeatedly (`render_max_dim`, `Session::geometry_mode`, the
thumbnail loader's shadow copy). One rule, always.

⚠ The consequence, stated so it is not later found as a bug: **the same Vertical value does not
mean the same physical camera pitch on a 16 mm shot and a 200 mm shot.** It means "this much
keystone", judged by eye — which is how Lightroom's manual Transform sliders behave too.

### Autoscale

Mirrors `Corrector::compute_autoscale` (`lens-profile/src/lib.rs:255`) rather than inventing a
second approach: probe the frame's four corners and four edge midpoints, map each back through
the inverse homography, and take the largest ratio by which a probe's source lands outside the
frame. `Perspective::scale` then interpolates `1.0 → autoscale` over 0..100, exactly as
`LensCorrection::scale` does.

⚠ A perspective correction empties the frame far more aggressively than pincushion distortion
does, which is why Scale defaults to 100 where it could reasonably default to 0.

## The stage

`PerspectiveStage`, in the **prefix**, between `LensCorrectionStage` and `GeometryStage`, keeping
the canvas size. New `PrefixStepKey::Perspective(Perspective)`.

Three things fall out of that placement, each of which would otherwise be work:

- ⚠ **No `Region`, no kernel margin, no `REGION_MARGIN` change**, despite being a spatial
  resample — the prefix always runs whole-buffer. Lens correction's property, for lens
  correction's reason.
- ⚠ **No `RenderScale`.** The transform is normalised, so preview and export agree exactly. The
  creative vignette's property.
- ⚠ **Its own cache checkpoint**, so dragging the crop, Straighten, or any tail slider never
  re-runs it.

Order within the prefix is forced: **after** lens correction, because a lens calibration is
defined in the uncropped sensor frame and describes the optics rather than the pose; **before**
geometry, because you correct the keystone, then level what is left, then crop away the corners
the correction emptied. Running it *after* the crop would move content around inside a rectangle
already chosen, leaving gaps inside the crop.

## ⚠⚠ `FrameMap` is untouched, and that is the reason for this placement

Brush strokes are anchored to **`GeometryStage`'s input**, and `FrameMap` maps them forward by
inverting that stage's transform. Sitting upstream of that anchor, perspective needs **no
`FrameMap` change, no `MaskStage` change and no mask change at all** — it inherits the caveat lens
correction already documents rather than requiring new code:

> ⚠ **Toggling perspective after painting shifts brush strokes.** Identical in kind to lens
> correction's accepted caveat (1), and larger in magnitude at strong settings.

The alternative considered and rejected was fusing perspective into `GeometryStage` as a single
homography with `straighten`. It gives better image quality — one bilinear resample rather than
two — but `FrameMap` would then have to invert a homography rather than a rotation, and that
agreement test is exactly what caught the rotation sign error in M3.1c-A. It also loses the cache
checkpoint. **Recorded as a backlog follow-up conditional on measuring visible softness at 1:1 on
a real photograph, not assumed away.**

## UI

The four sliders go in the **Crop rail tool**, below Straighten, under a `Perspective` label.

⚠ That is where they can be judged: **crop mode previews the uncropped frame** (`preview_edits`
resets the crop), which is the only view where the empty corners are visible at all. This is the
identical argument that already puts the Lens `Scale` slider in this tool, and it applies with
more force here because keystone empties far more of the frame.

**The rail entry is renamed `Crop` → `Geometry`.** Its hover text already reads *"Crop &
Geometry"*, the edit group is already `Geometry`, and the panel module is already `geometry.rs` —
so the name finally agrees everywhere. The glyph and the `2` shortcut are unchanged.

⚠ A **Reset perspective** control is not added. The tool's existing reset button is scoped to the
Geometry *group* (`GEOMETRY_GROUP`), and perspective is in that group, so it is already covered —
adding a second reset would be two controls whose scopes overlap.

⚠⚠ **But that button's label and hover text go stale, and must move in the same change.** It
currently reads "Reset crop", hovering *"Resets the crop, the straighten angle and any rotation or
flip"* — an enumeration that becomes wrong the instant a fifth thing joins the group. This is the
same failure as the "Reset adjustments" hover text that named five panel sections literally and
went wrong when a sixth was added; that one was fixed by **deriving** the text from `SECTIONS`.
There is no equivalent table here, so the fix is to make the text stop enumerating: the button
becomes **"Reset geometry"**, hovering *"Resets the crop, straighten, rotation, flip and
perspective. Leaves the adjustments and any masks alone."* ⚠ It still enumerates, so the rule for
the next person is written at the call site: **anything added to `GEOMETRY_GROUP` must be named
here.**

## Testing

- **The identity is exact.** At neutral the stage is not built at all (`is_active`), and every one
  of the 47 goldens must stay byte-identical. That is the evidence the change is scoped.
- **Verticals actually become parallel.** Construct a source with two lines converging to a known
  vanishing point, apply the `θ` the maths predicts, and assert the corrected lines are parallel
  to within a tolerance. ⚠ **This is the test that must not be a golden**: a golden only ever
  catches a *change*, and would bake a wrong convention into its reference on the first run — the
  lesson `the_tint_lands_at_the_hue_the_wheel_names` was added for.
- **The homography round-trips.** `H⁻¹(H(p)) == p` for probe points with leverage. ⚠ Probes must
  sit far from the centre: a perspective error displaces a point in proportion to its distance
  from the axis, and the M3.1c-A finding was a rotation sign error that survived because its probe
  sat at 0.34 of the half-width.
- **Sign conventions, one test per slider.** Positive Vertical must correct a building that leans
  *back*; a test that only checks "something moved" cannot tell a sign error from a correct one.
- **Autoscale bounds the frame.** After scaling at 100, no output pixel maps outside the source.
  ⚠ And its complement: at scale 0 a strong keystone genuinely *does* leave empty corners, or the
  test is measuring nothing.
- **Aspect is area-preserving to first order**, which is what `diag(s, 1/s)` claims.
- **Two goldens**, on a chart with strong straight-line structure: `perspective_vertical` and
  `perspective_aspect`. ⚠ **Measured, not eyeballed** — the chart must not clip, on the rule the
  colour-grading goldens established, and the two must differ from one another or one of the axes
  is inert.
- **Sabotage list, run and recorded**: swap the pitch and yaw axes; negate one rotation only (a
  pair negated together stays self-consistent and the round trip survives — the trap hit twice in
  the colour-management work); drop the `is_active` scale exclusion; derive `Default`; remove the
  autoscale interpolation.
- ⚠ **Not tested here:** that the panel wires the sliders, and that the rail rename took. Those are
  wiring, verified by hand with the app running — the honest boundary this project's headless
  canvas tests draw.

## Accepted caveats

1. **No guided control points** — the more powerful half, and a separate slice.
2. **No automatic vertical detection** — needs Hough line detection, a new subsystem.
3. **The assumed focal length is fixed**, so the same slider value is not the same physical pitch
   on different lenses. Aspect is the complement.
4. **Toggling perspective after painting shifts brush strokes**, as lens correction already does.
5. **A third bilinear resample** in the chain (lens → perspective → straighten) softens slightly.
   Fusing is on the backlog, conditional on the softness being measurable.
6. **No X/Y offset** — the crop rectangle, applied immediately afterwards, already chooses which
   part survives.
7. **No separate Rotate** — the existing Straighten slider is it, and a second would be two facts
   that can disagree.
8. **Strong corrections lose real resolution**: the correction stretches one end of the frame and
   Scale then crops in, so the result is softer at the stretched end. Inherent to keystone
   correction, not a defect.
