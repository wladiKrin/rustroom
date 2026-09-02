# M5.4b — a display rendering in a scene-referred pipeline

*Design, 2026-09-02.*

A RAW opens looking like the photograph the camera made, with no file the user had to find. The
mechanism is one new stage, and it is the seam three other backlog items have been waiting on.

---

## 1. Why, and the measurement that shaped it

`OutputTransform` is a *colorimetric* conversion — ProPhoto→sRGB primaries plus the transfer curve.
It is not a rendering: nothing maps the scene's tones onto a display's. So an unedited RAW opens
flat, and the filmstrip — which shows the camera's **embedded JPEG** — disagrees with the canvas.

Measured on the author's own files (`strip_vs_canvas`, kept):

| frame | strip (camera JPEG) | canvas (ours) | |
| --- | --- | --- | --- |
| DSCF6509 | 0.3475 | 0.2645 | **+0.39 EV** |
| DSCF6383 | 0.4120 | 0.3120 | **+0.40 EV** |
| DSCF6489 | 0.2163 | 0.1998 | +0.11 EV |

⚠⚠ **And the camera's transfer was measured directly rather than assumed** (`curve_probe`, kept),
by pairing a scene-linear decode against the embedded JPEG of the same frame:

| scene linear | camera | ratio |
| --- | --- | --- |
| 0.0027 | 0.0006 | **0.23×** |
| 0.0048 | 0.0009 | 0.19× |
| 0.0087 | 0.0029 | 0.33× |
| 0.0157 | 0.0363 | **2.31×** |
| 0.0283 | 0.0441 | 1.56× |
| 0.0511 | 0.0746 | 1.46× |
| 0.0921 | 0.2102 | 2.28× |

**It is a true S-curve: it crushes below ~0.01 and lifts hard above.** ⚠ That measurement killed
the family this design was going to use. A compressive curve `y = xᶜ/(xᶜ + k)` has a *monotone*
log-log slope, so it can crush or lift but not both: matching the 0.19× toe forces `c ≈ 1.67`,
which then gives 1.18× at 0.09 where the camera gives 2.28×. **A rendering has to be a sigmoid in
log-exposure space**, which is what ACES, Filmic and AgX all are, and the reason is now on record
rather than borrowed.

---

## 2. Placement — between Sharpening and `OutputTransform`

```
… masks → vignette → sharpen → DisplayRenderingStage → [future: creative LUT] → OutputTransform
```

- **Everything upstream stays scene-linear**, so every constant tuned against linear luminance
  survives untouched: `local_contrast`'s `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS`/`GAIN_MAX`/
  `HALO_KNEE`, `dehaze`'s airlight percentile. That is precisely why the backlog refused to bake a
  curve into the decode.
- **Masks still run in linear**, where a masked exposure means what it says.
- ⚠ **It sits after sharpening, and `CLAUDE.md`'s "sharpening is last so nothing stretches it"
  survives that.** `OutputTransform` was *always* after sharpening, and an sRGB encode is a ~1/2.2
  power — far more aggressive than any tone curve. The principle means "last among capture
  stages", not "last before any non-linearity". Adding a smooth curve there is a marginal change,
  not a new class of problem.
- **⚠⚠ It maps `[0, ∞) → [0, 1]`, and that bounding is what unblocks the other three items.** The
  backlog's stated blocker for creative LUTs is *"the cube is bounded and our data is not"*. A
  rendering is exactly the thing that bounds it. `ProfileLookTable` wants to run immediately after
  the tone curve — the same slot. Neither is built here; the seam is.
- **The histogram is correct for free.** `pack` quantizes the pipeline's output and takes the
  histogram from that, so the histogram describes the *rendered* image — which is what a clipping
  instrument must do.

---

## 3. The curve

**A monotone cubic spline over log-exposure**, `t = log2(x / GREY)` with `GREY = 0.18`.

⚠⚠ **It reuses the Fritsch–Carlson spline `ToneCurveStage` already has.** That spline is currently
`struct Spline`, private to `stages/tone_curve.rs`; it moves to `pipeline/src/spline.rs` as
`pub(crate)` with `X_EPS`, `FLAT_EPS` and `end_tangent`. **`tone_curve.rs`'s existing tests must
pass unchanged** — that is the evidence the move changed no behaviour. Monotonicity matters here
for the reason it matters there: an unconstrained cubic overshoots at a knee, and in a tone curve
an overshoot is a contrast reversal.

**Applied per channel** (user's call): R, G and B each through the same curve. That is where the
shadow saturation and the roll toward white come from, and it is what makes a rendering look like
a camera rather than like a gain. ⚠ A deliberate departure from this codebase's hue-preserving
convention — `ToneCurveStage`, `ToneRegionsStage` and the rest curve luminance and apply a scalar
gain — justified because **a rendering is not an adjustment**. Say so at the stage.

**Outside the control points:** below the first, output 0; above the last, output 1. Both clamp,
so the stage is total on `[0, ∞)` and can never produce a negative or an out-of-range value.

**The control points are fitted, not invented.** The implementation plan runs `curve_probe` over
several frames spanning dark to bright, pools the (scene, camera) pairs, and fits the spline's
points to them. ⚠ The table above is one *dark* frame and covers only `t < 0`; **a bright frame is
required** or the curve above grey is extrapolation. Provisional points go in the code with a
comment saying they are provisional until the fit replaces them.

**Compiled to a LUT at construction**, as `ToneCurveStage` does — the log, the spline evaluation
and the clamps are all per-pixel work otherwise, three times over for three channels.

---

## 4. The model

```rust
/// How a scene-referred photograph is mapped onto a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Rendering {
    /// Colorimetric only — what Rustroom did before this existed.
    #[default]
    None,
    /// The camera's rendering: the profile's tone curve when it has a usable one, else the
    /// built-in filmic curve.
    Camera,
}
```

`Edits.rendering: Rendering`, `#[serde(default)]`.

- **⚠⚠ `None` is the default, and that is what keeps the harness intact.** `Edits::default()` stays
  strictly neutral, so **all 53 goldens are untouched** and the CLI renders exactly as it does
  today. `Settings.default_rendering` (default `Camera`) is applied to a photo with **no sidecar
  yet**, exactly as `auto_lens_correction` already works. ⚠ Accepted consequence, stated because it
  is the same one lens correction carries: **the GUI and the CLI disagree on a fresh photo.**
- **⚠ Its own edit group, `"Rendering"`**, appended to `EDIT_GROUP_NAMES`, **included** in
  `EditGroups::default()` so it travels with a paste, a preset and a sync — it is a look, not a
  placement.
- **⚠⚠ No panel *section* is named "Rendering", and that is deliberate.** The "Reset adjustments"
  button derives its group set from `SECTIONS`, so a group with no matching section is **not
  reset** — which is what stops "Reset adjustments" flattening the photograph. The control is drawn
  at the top of the **Tone** section, above Auto, because it is the baseline the tonal sliders sit
  on. ⚠ `geometry_and_masks_are_the_two_groups_the_panel_does_not_show` must gain `"Rendering"`.
- **A checkbox**, since the enum has two variants. An enum rather than a `bool` so a third
  rendering is not a migration; when one arrives the checkbox becomes a combo.

---

## 5. Where the profile's curve comes from

⚠ **`ProfileToneCurve` is not parsed today** — checked: `camera-profile` has no tone-curve tag at
all. This adds DNG tag **50940** as `Profile.tone_curve: Option<Vec<[f32; 2]>>`.

It reaches the pipeline the way `lens` does — **captured at `RenderEngine::new`**, not passed per
request, because it cannot change without a re-decode (the profile is a `Settings` choice that
rebuilds the decode worker). `Decoded` carries it, `Session` holds it, and it becomes a **seventh
parameter** to `build_pipeline_split_in_region_with_brush` beside `lens`. ⚠ **Not a `RenderSpec`
field**: `RenderSpec` is `Copy` and a curve is not, which is exactly why `lens` sits outside it. An
eighth parameter would trip `too_many_arguments` and want a struct; seven does not.

**⚠⚠ An identity curve is ignored.** The X-T4 profile's `ProfileToneCurve` is `[0,0,1,1]` — the
identity — so honouring it literally under `Rendering::Camera` would produce **no rendering at
all**, which is indistinguishable from `None` and defeats the setting on the author's own camera.
The rule: *use the profile's curve only when it is not the identity; otherwise the built-in.* It
has its own test, because it is the difference between the feature working and silently not.

⚠ The profile's curve is defined on `[0, 1]`, so its input is **clamped** there. Scene values above
1.0 map to white. The built-in curve does not clamp — it is total on `[0, ∞)`. Stated as a caveat
rather than papered over.

---

## 6. RAW only

**⚠⚠ The stage is inserted only for a scene-referred source.** A JPEG, PNG or TIFF has *already*
been rendered by whatever produced it, and so has the camera's embedded thumbnail — applying ours
on top would render twice. Concretely:

- `image-decode` sources: no rendering. The decoder is the only thing that knows, so the flag rides
  with the decode: `Decoded.scene_referred: bool`, true only for `raw-decode`.
- **Thumbnails: no rendering.** `thumb_loader::render_edits` runs the pipeline over the camera's
  embedded JPEG, so it passes `Rendering::None` regardless of what the photo's `Edits` say.
- ⚠ **This is what closes the strip/canvas gap** rather than widening it: the strip keeps Fuji's
  rendering, the canvas gains ours, and the two converge. It is also why the acceptance test in §8
  is meaningful.

⚠ A non-RAW photo still *stores* a `rendering` value (the `Settings` default writes one), and it is
simply ignored. Harmless, and it means a paste from a JPEG onto a RAW carries a sensible value
rather than `None`.

---

## 7. Tests

**`pipeline::spline`** — `tone_curve.rs`'s existing tests pass **unchanged** after the extraction.

**`stages::display_rendering`**
1. **`Rendering::None` inserts no stage**, and a neutral render is **bit-exact**. The rule the whole
   golden harness rests on; sabotage by always inserting and watch the goldens fail.
2. **Total and monotone**: sweep `x` over `[0, 64]` including 0 and values far above 1 — output is
   always in `[0, 1]` and never decreases. No NaN for any input including `f32::MAX`.
3. **The measured shape**: the curve crushes below ~0.01 (ratio < 1) and lifts around 0.05–0.1
   (ratio > 1). ⚠ **This is the test that can see a curve fitted with the wrong sign or in linear
   instead of log space** — a monotone-and-total test passes against a plain ramp.
4. **Per channel, not luminance**: a saturated colour's channel ratios *change*; a neutral grey
   stays exactly neutral. Both halves — a neutral-only test passes against a luminance model.
5. **The identity profile curve is ignored** and the built-in used instead; a non-identity one is
   used. Both directions.
6. **A profile curve clamps above 1.0** and the built-in does not.

**`camera-profile`** — tag 50940 parses from a synthetic DCP; a profile without one yields `None`;
a malformed one is an `Err`, never a panic.

**`app`** — a photo with no sidecar gets `Settings.default_rendering`; one *with* a sidecar keeps
what it had; a **non-RAW** photo renders with no rendering stage even when its edits ask for one;
a thumbnail never renders through it.

**`edit-model`** — `merge_from` moves `rendering` under the `Rendering` group and nothing else;
`EditGroups::default()` includes it.

⚠ Sabotage every test, and confirm each **ran**, **compiled**, and **actually changed the file** —
`cargo fmt` reflows, so a text match can apply to nothing.

---

## 8. Acceptance — measurable, then judged

**Measured:** `strip_vs_canvas` runs on at least three frames before and after. The gap must
**collapse** from +0.39 / +0.40 / +0.11 EV toward zero. That is the number the fit optimises and
the one to report.

**Judged:** the user signs off the photographs. The implementation produces before/after JPEGs on
three frames spanning dark to bright, and **stops** if the curve looks wrong rather than tuning it
further on its own — the rule `PARALLEL-WORK.md` §6 sets for anything whose "done when" is a
judgement about an image.

⚠ A mean-luma gap of zero does not prove the *shape* is right, only the average. Report the binned
comparison from `curve_probe` as well, so a curve that is right on average and wrong in the
shadows is visible.

---

## 9. Accepted caveats

1. **The GUI and the CLI disagree on a fresh photo** — the `auto_lens_correction` trade, taken
   again knowingly.
2. **A profile tone curve clamps above 1.0**; the built-in does not.
3. **`ProfileToneCurve` has no visible effect on the author's camera**, whose curve is the identity.
   It ships unit-tested against a synthetic profile and unverified against real gear — the position
   C2/C3 were in without an oracle, stated rather than discovered later.
4. **`ProfileLookTable` and creative LUTs are not built.** The seam is, and it bounds the data,
   which is what they were blocked on.
5. **The built-in curve is one curve for every camera.** Fuji's film simulations differ from
   Canon's and from each other; this approximates a generic camera rendering, not a specific one.
6. **Per-channel curving shifts hue on saturated colour** — that is the intent, and it is the one
   thing most likely to be reported later as a colour bug.
7. **No user control over the curve's shape.** The tone curve the user already has sits upstream in
   scene-linear and is a different instrument.
