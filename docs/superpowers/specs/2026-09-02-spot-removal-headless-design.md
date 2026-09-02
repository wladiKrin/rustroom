# M5.3a — Spot removal, headless

*Design, 2026-09-02. Wave 3, track A.*

Clone and heal patches as an `Edits` field and a pipeline stage, proved against goldens and a real
`.RAF` with **no canvas code in existence**. The canvas tool is 5.3b; content-aware fill is 5.3c.
This follows the M3.1a/M3.1c-A split exactly: the whole model first, the gesture later.

---

## 1. The model

```rust
/// One retouch patch: pixels copied from `source` onto `centre`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    /// Destination centre, normalised per axis over the pre-geometry frame.
    pub centre: [f32; 2],
    /// Source centre, same space.
    pub source: [f32; 2],
    /// Radius as a fraction of the pre-geometry frame's **long edge** — `BrushStroke::radius`'s
    /// rule, for its reason: `centre` lives in an anisotropic space, and a radius measured against
    /// width but compared against height would make every patch an ellipse.
    pub radius: f32,
    /// 0 = hard edge, 100 = the ramp runs from the centre to the rim. `RadialGradient::feather`'s
    /// convention and its smoothstep.
    pub feather: f32,
    pub mode: SpotMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpotMode {
    /// Straight copy, blended by the feather.
    Clone,
    /// Clone with the source patch's mean RGB shifted to match the ring around the destination —
    /// what makes a spot on a sky *disappear* rather than leave a disc of slightly different sky.
    #[default]
    Heal,
}
```

`Edits.spots: Vec<Spot>`, `#[serde(default)]`, empty by default. **Circles only** (user's call): a
stroke-shaped spot is a later slice if ever wanted.

⚠⚠ **`Spot` stays `Copy`.** `Mask` lost `Copy` for the brush's stroke list and three call sites
paid for it; a spot is five numbers and an enum.

### Edit group

A new group **"Retouch"**, appended as `EDIT_GROUP_NAMES[8]` with a `RETOUCH_GROUP` constant, serde
field `retouch`. **Default OFF** for paste, presets and sync — Lightroom's Copy Settings excludes
Spot Removal by default for the same reason it excludes crop and masks: the values are
content-referred. Syncing across a burst with the box ticked is still the real use — the same
sensor dust sits at the same coordinates on 200 frames — and pre-geometry anchoring is exactly what
makes that correct.

⚠ **Adding a field means all five exhaustive sites**: `merge_from`'s destructure, `EditGroups` (and
its `only`/`get`/`get_mut`, the `EDIT_GROUP_NAMES` length), `pipeline::all_edits`,
`edit_model::all_nonneutral`, and `app::edit_clipboard`'s payload fixture. The compiler names each
one; that is what they are for. Do not weaken any of them.

---

## 2. Placement — after perspective, before geometry

`build_prefix_steps` runs `LensCorrection → Perspective → Geometry → NoiseReduction → Dehaze`.
`SpotRemovalStage` goes **between Perspective and Geometry**, with its own
`PrefixStepKey::SpotRemoval(Vec<Spot>)`.

Why exactly there, and not anywhere else:

- **It is `GeometryStage`'s input, which is where brush strokes are anchored.** A spot is
  content-referred — a dust spot is on the sensor, a blemish on the subject — so it must not slide
  when the crop changes. Normalising against the frame `GeometryStage` receives makes *"the patch
  survives a crop"* free, **and** `FrameMap::inverse_point` already maps a canvas click into this
  space for 5.3b with no new code. ⚠ It inherits the brush's caveat: toggling lens correction or
  perspective after placing a spot shifts it. Record it; do not fix it here.
- **Before noise reduction and dehaze**, so a healed patch is denoised with its surroundings rather
  than showing a different grain.
- **In the prefix**, so it always runs whole-buffer: no `Region`, no kernel margin, no
  `tail_kernel_radius` change, and every tail slider stays cached while spots are edited.
- Its own `PrefixStepKey`, so adding a spot re-runs only spot removal and what is downstream —
  never lens correction, the expensive stage.

⚠ **No stage when `spots` is empty.** Neutral must be bit-exact, not trusted: the `kelvin: None`
rule, and the reason all 51 goldens stay byte-identical.

---

## 3. The stage

For each spot **in order**, on the working buffer:

1. Pixels: `cx = centre.x · (w − 1)`, `cy = centre.y · (h − 1)`, likewise for the source;
   `r = radius · max(w, h)`.
2. **The offset is rounded to whole pixels.** `dx = round(sx − cx)`, `dy = round(sy − cy)`.
   ⚠ Deliberately not sub-pixel: a resampled patch is *softer* than its surroundings, and that
   softness is precisely what makes a patch visible. A dust spot does not need half-pixel placement.
3. **Gather the source disc into a scratch first**, then blend into the buffer. That is what lets a
   later spot source from an earlier spot's healed area — Lightroom's behaviour, and useful — and
   what stops a spot ever reading half-written pixels when its source overlaps its destination.
   Source pixels outside the image clamp to the edge.
4. Coverage at a destination pixel at distance `d` from the centre, with `f = feather / 100`:
   `1` for `d ≤ (1 − f)·r`, smoothstep down to `0` at `d = r`. ⚠ At `f = 0` the ramp width is zero
   — guard the division exactly as `RadialGradient::FEATHER_EPS` does, and note that this guard is
   **load-bearing** (the radial one is; the brush's `HARDNESS_EPS` is not — the comment there draws
   the contrast).
5. **Heal** (default): `ring_mean` = mean RGB of the destination annulus `r < d ≤ 1.5r` on the
   buffer *before* this spot is applied; `src_mean` = mean RGB of the gathered source disc.
   `patch = src + (ring_mean − src_mean)`, per channel, clamped at 0 (scene-referred, so no upper
   clamp). **Additive in linear light**, not multiplicative: what a sky gradient carries across a
   spot is an offset in exposure, and a ratio would over-correct dark sources.
   **Clone**: `patch = src`.
6. `out = lerp(dst, patch, coverage)`.

Per-pixel and position-independent within a disc, so `map_pixels_xy` is *not* the shape — a
gather. Write a plain loop over the disc's bounding box; rayon is not worth it for a few thousand
pixels per spot.

`RenderScale` is not needed: coordinates and radius are normalised, so preview and export agree
exactly.

---

## 4. Tests

In `pipeline/src/stages/spot_removal.rs`:

1. **Neutral is no stage** — `build_prefix_steps` with empty `spots` has no `SpotRemoval` key.
2. **A hard-edged clone is a bit-exact copy inside the disc** at `feather: 0`, and bit-exact
   untouched outside it. Both halves; one alone passes against a stage that copies the whole image.
3. **Heal matches the ring.** A source disc on a dark background healed onto a bright one lands
   with its mean equal to the ring's mean (to 1e-5), where clone would not — assert both, so the two
   modes are distinguishable.
4. **A later spot can source from an earlier one's result**, and reads none of its own half-written
   pixels when source and destination overlap (the gather-first rule).
5. **A source off the image clamps** rather than panicking or reading garbage.
6. **The patch survives a crop** — the shape of the brush's real-`.RAF` validation: render with and
   without a crop, and the healed pixels land on the *same source content* (compare the crop's
   window of the uncropped render against the cropped render, bit-exactly).
7. **Feather ramps monotonically** from full at the inner radius to zero at the rim, and the
   `feather: 0` guard: probe **at** `d == r` exactly, the case `feather_zero_is_a_hard_edge` in
   `radial` records IEEE infinity hiding.
8. `merge_from` transfers spots under `RETOUCH_GROUP` and nothing else; `EditGroups::default()`
   excludes it.

**Two goldens**, `spot_clone` and `spot_heal`, on a new chart: a smooth vertical gradient with a
dark 6 px disc "dust spot" and a spot placed over it with its source 3 radii to the side. Heal must
leave the gradient continuous through the patch; clone must show the source's brightness. ⚠
**Measure the generated goldens** — no clipping, and `spot_clone` ≠ `spot_heal` (the two-goldens-
differ test the grading goldens carry). ⚠ You are the only track adding goldens this wave; D
*re-authors* existing ones. Merge after D.

**Real `.RAF` validation**, hand-written sidecar through `rustroom render --edits`, on
`~/Desktop/fuji/DSCF6509.RAF` or a frame with a visible sensor spot: measure the mean over the
destination disc before and after, and the ring, and report the numbers — the brush's
`(0.358, 0.394)` style of evidence, not a screenshot.

**Sabotage every test**, per `docs/PARALLEL-WORK.md`. Predicted blind spots to check in advance:
a test that only probes the disc *centre* cannot see a wrong radius; a clone test on a uniform
source cannot see a wrong offset.

---

## 5. Ownership

**Yours:** `edit-model/src/lib.rs` (field, `Spot`, `SpotMode`, group), the five exhaustive sites,
`pipeline/src/lib.rs` (stage insertion + `PrefixStepKey`), new
`pipeline/src/stages/spot_removal.rs`, `pipeline/tests/golden.rs` (+2 cases, appended at the end)
and two new PNGs, a probe example under `crates/app/examples/` if you want one.

**Not yours:** anything in `develop/` (panel, canvas), `Settings`, `catalog`, `local_contrast.rs`
(D), `adjustments.rs` (C). 5.3b — the canvas tool, source auto-placement, the panel list — is
Wave 4.

---

## 6. Accepted caveats

1. No canvas tool: a spot is authored only by hand-editing a sidecar until 5.3b — the cost the
   M3.1a split paid too.
2. Circles only.
3. Whole-pixel offsets.
4. Spots shift if lens correction or perspective is toggled afterwards — the brush's caveat.
5. Heal matches the **mean** only; a source on a different texture still reads as a patch. That is
   what 5.3c is for.
6. Spots run before noise reduction, so a very noisy source is denoised with its surroundings —
   correct, and also means a spot cannot be judged at fit resolution any more than sharpening can.
