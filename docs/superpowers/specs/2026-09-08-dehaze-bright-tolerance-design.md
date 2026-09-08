# Dehaze — an airlight tolerance for bright achromatic subjects — design

**Status:** specced 2026-09-08. Branch `m7.6-dehaze-bright`. One of the two M7.6 gaps that were
genuinely open (the other is auto-mask for the brush).

Dehaze reads a bright achromatic object — snow, a white wall, a white cat — as haze, because its
dark channel is high. That is the classic dark-channel-prior artifact, recorded as accepted
caveat 1 of `2026-08-11-dehaze-design.md`. This change makes the stage treat a pixel whose colour
is **within a tolerance of the airlight** as an object of that colour rather than as haze, by
raising its transmission toward 1. A fixed constant, no slider.

---

## 1. Measured first, and the measurement reshaped the brief

The backlog said the fix *"needs large-bright-region detection and is its own feature"*. Before
designing anything, a probe (`dehaze_bright_probe`, kept) rendered every one of the **415 RAWs**
in the author's folder at Dehaze +60 and +100, under the camera rendering, and scored the
**bright achromatic** pixels of each frame (encoded max channel > 0.6, saturation < 0.12, judged
on the neutral render): their mean relative darkening, the worst decile, and their Oklab chroma
gain. Four things came out of it, and each moved the design.

1. **At +60 the artifact is mild.** Over the 183 frames with at least 3% bright achromatic
   pixels: median darkening 0.8%, 90th percentile 2.8%, worst 6.1%. **At +100 it is not**: median
   1.4%, but the worst frames darken **15–19% on average and 38–48% in their worst decile**, with
   a chroma gain of +0.013 — a visible blue-grey tint. Both white-kitten frames (DSCF6546,
   DSCF6557) show a dirty grey wall with a dark ring toward the corners.
2. **⚠⚠ The ring is flash falloff read as depth.** The airlight is estimated from the brightest
   region — the wall itself, a warm white — and every wall pixel darker than that estimate is
   pushed further from it by `(I − A)/t + A`. The subtraction of a warm airlight is what makes
   the darkened wall go blue.
3. **⚠⚠ This catalogue contains no genuinely hazy landscape.** Ranked by how much Dehaze changes
   the whole frame, the top ten are all kittens in front of a flat lilac wall, which +100 turns
   saturated violet. So the "haze must still be removed" regression set is the **synthetic**
   scene the stage's own tests already use, not a photograph. Stated so nobody later looks for a
   real-frame check that cannot exist here.
4. **Cloudy skies are a different case from white walls.** DSCF6296 (abbey) and DSCF6236 darken
   ~15% at +100 — but their bright pixels are *not* near the airlight, and at +60 the sheets show
   the effect people reach for Dehaze to get: a bluer sky and more cloud contrast. **Not treated as
   an artifact.**

## 2. The rule

In `transmission`, after the dark channel and **before** the guided refinement:

```rust
/// Distance from the airlight within which a pixel is read as an object of that colour, not as
/// haze. Linear working-space units.
const AIRLIGHT_TOLERANCE: f32 = 0.1;

let dist = (I_r − A_r).abs().max((I_g − A_g).abs()).max((I_b − A_b).abs());
let t = 1.0 − OMEGA * dark;
let t = (t * (AIRLIGHT_TOLERANCE / dist.max(1e-6)).max(1.0)).min(1.0);
```

- **The distance is the largest per-channel difference**, so a pixel counts as "the airlight's
  colour" only when *every* channel is within K. It is what the sweep measured; a Euclidean
  distance was not tried and is not proposed.
- **The curve is continuous.** Below `dist = K·t` it is 1; between `K·t` and `K` it is `t·K/dist`,
  which reaches exactly `t` at `dist = K`; above `K` it is `t`. There is no jump at either
  breakpoint, only a kink in the derivative, and running **before** the guided refinement smooths
  that kink spatially the way the refinement already smooths the patch-minimum's blocks.
- **A fixed constant, no slider.** Lightroom's Dehaze exposes no such control, and the Clarity
  halo suppressor set the precedent (its floor and knee are constants): this is part of what the
  tool *means*, not a decision to hand over. ⚠ The spike read K from an environment variable so a
  probe could sweep it; that variable is **not** shipped.
- **Per photo, because the airlight is.** The same wall is exempt in one frame and not in another
  if the airlight estimate differs — inherent, and the same property caveat 3 of the original
  spec already records for the whole stage.

## 3. What it buys and what it costs — the sweep

Six frames at +100, `RUSTROOM_DEHAZE_K` swept on a build carrying the rule:

| frame | K = 0 (today) | 0.06 | **0.1** | 0.2 |
|---|---|---|---|---|
| white wall, kitten (DSCF6546) | 18.7% darker, chroma +0.0135 | 10.4% | **3.4%, +0.0013** | 0.0% |
| white wall, kitten (DSCF6557) | 15.7%, +0.0130 | 3.8% | **0.8%, +0.0001** | 0.0% |
| cloudy sky, abbey (DSCF6296) | 15.1% | 15.2% | 15.0% | 8.9% |
| cloudy sky, couple (DSCF6236) | 15.3% | 14.7% | 13.0% | 3.1% |
| wall behind a portrait (DSCF6327) | 8.5% | 8.5% | 8.4% | 8.4% |
| synthetic haze residual after removal | 0.336 | 0.36 (at 0.05) | **0.380** | — |
| the nine dehaze tests | pass | pass | **pass** | `negative_dehaze_is_depth_aware` FAILS |

- **K = 0.1 removes the white-wall case** — 18.7% → 3.4% and 15.7% → 0.8%, the tint to zero.
- **⚠ It costs 13% of haze removal on the synthetic scene**: the residual after +100 rises from
  0.336 to 0.380 of the haze, against `synthetic_haze_is_removed`'s bound of 0.5. Dense true haze
  *is* near the airlight by definition, so some of it is now read as object. That is the trade, and
  the test's bound is what keeps it honest.
- **⚠ 0.2 is past the ceiling**: at that tolerance the depth gradient of a flat scene is swallowed
  and the depth-aware test fails. 0.1 sits at roughly half of it.
- **Cost: none measurable** — `bench_preview`'s dehaze-60 row read 42.2 ms without and 40.5 ms
  with, i.e. inside the run-to-run noise. One more per-pixel pass over a buffer the stage already
  traverses twice.
- **Two cases are left alone, on purpose, and recorded as limits**: cloudy skies (§1, item 4),
  and a wall whose frame's airlight is a *different* bright region — DSCF6327's wall sits behind a
  brighter window, so the wall is not near the airlight and the tolerance cannot reach it. A fix
  for that shape would be region detection, which §1 says cannot be checked against a real hazy
  frame here and which the backlog entry can carry forward.

## 4. Goldens

Three references move, because the goldens' hazy region is by construction near the airlight:
`dehaze_plus` by **19** of 255 at its worst sample, `dehaze_minus` by **5**, `mask_dehaze` by
**4**. All three are **re-authored deliberately** with `UPDATE_GOLDEN=1`, and the other **56 must
come out byte-identical** — that is the proof the change is confined to this stage. ⚠ The
existing guard `a_masked_capture_correction_really_changes_the_chart` must stay green on the
re-authored `mask_dehaze`; if the tolerance took enough of the effect to fail it, that would be a
finding about K, not a reason to weaken the guard.

## 5. Tests — and the sabotage each catches

- **`a_bright_object_near_the_airlight_is_left_alone`** — the artifact as a test. A hazy synthetic
  scene with a large flat patch at **0.92 × A** (slightly darker than the airlight, the white-wall
  case; a patch at exactly A is unchanged by the stage with or without the tolerance, since
  `(A − A)/t + A = A`). At +100 the patch's mean luminance must stay within **2%** of its input.
  The premise is asserted through a test-only entry point taking the tolerance:
  `transmission_with(image, a, scale, tolerance)` — `transmission` delegates with the constant,
  the `apply_with(halo_floor)` precedent — and at tolerance 0 the same patch must darken by more
  than **10%**. Sabotage: the constant set to 0 fails the first half.
- **`the_tolerance_is_continuous`** — transmission over a row of pixels whose distance from the
  airlight ramps across both breakpoints, with the dark channel held constant: no two adjacent
  pixels differ by more than a small epsilon. Sabotage: replacing `max(1.0)` with a hard branch
  (`if dist < K { 1.0 } else { t }`) fails it.
- **`synthetic_haze_is_removed`**, **`negative_dehaze_is_depth_aware`** and
  **`a_clear_image_is_barely_changed`** stand unchanged and must pass — the second is the ceiling
  test, and §3 records why 0.2 would fail it.
- **`the_dehaze_goldens_still_discriminate`**, only if the re-authored `dehaze_plus` is found to
  be close to neutral: measure the re-authored PNGs against `neutral`-equivalent renders (the
  practice the grading and spot goldens follow) rather than trusting the eye.
- The probe **`dehaze_bright_probe` is kept** as a tool, its stem filter documented rather than
  marked as a spike, its environment-variable coupling removed.

## 6. Where the code goes

| change | file | lines now |
|---|---|---|
| the constant, the distance, the rule, `transmission_with` | `crates/pipeline/src/stages/dehaze.rs` | 422 |
| three re-authored PNGs | `crates/pipeline/tests/golden/` | — |
| the probe, finalised | `crates/app/examples/dehaze_bright_probe.rs` | new |
| records | `CLAUDE.md`, `docs/BACKLOG.md`, `2026-08-11-dehaze-design.md` caveat 1, the vault's M7.6 entry | — |

No `Edits` change, no `EditGroups`, no panel, no cache key — the stage's parameters are
unchanged and the tolerance is a constant. Masked dehaze inherits it through the same stage.

## 7. Accepted caveats

1. **Haze removal is ~13% weaker on dense haze near the airlight** (0.336 → 0.380 residual on the
   synthetic scene). The price of not darkening a white wall, and the bound that guards it is 0.5.
2. **Cloudy skies still darken ~15% at +100** — deliberately not treated as the artifact.
3. **A bright object whose frame's airlight is a different bright region is untouched**
   (DSCF6327). Region detection would address it and is out of scope for the reasons in §1.
4. **K is derived from six X-T4 frames and one synthetic scene**, at the 0.1 the sweep found;
   the ceiling (0.2) is measured, the floor (0.06 buys half the benefit) is measured, and 0.1 is
   the smallest value that removes the white-wall case.
5. **The exemption is per photo**, like the airlight.
6. **Three goldens move**, and this is the second time the dehaze goldens have been re-authored
   for a model change.
