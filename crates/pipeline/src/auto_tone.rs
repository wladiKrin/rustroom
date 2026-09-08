//! Auto Tone — exposure, whites and blacks from the photograph's own histogram.
//!
//! The predictable, levels-style auto: put the black point, the white point and the middle
//! where they belong, and touch nothing else. ⚠ **This is M5.4a only.** A default camera
//! rendering — *where a display rendering lives in a scene-referred pipeline* — is 5.4b, a
//! design question reserved for the main session. Nothing here may grow into it.
//!
//! Pure: no I/O, no GUI. Luminance is [`color::luminance`], the working space's Y, the quantity
//! every tuned constant in this pipeline is measured against.
//!
//! ⚠⚠ **Whites and blacks are solved against the real [`ToneRegionsStage`], never a formula.**
//! Bisection pushes a neutral stand-in through the stage itself, so the auto agrees with the
//! sliders by construction — and if the tone-regions model ever changes, the auto follows it
//! with no edit here. The "one rule" principle.

use crate::{ExposureStage, PipelineStage, ToneRegionsStage};
use core_image::ImageBuf;
use edit_model::CropRect;

/// Where the median lands, in linear light.
///
/// ⚠ **0.20, re-judged under the camera rendering (user's call, 2026-09-08)** — the M5.4b
/// leftover. 0.22 had been chosen on 2026-09-02 when nothing followed Auto Tone; with the
/// S-curve now after it the results read *"a tad too bright, but not always"*, and one step
/// down (−0.14 EV) *"looks right"*. The "not always" is the frames' own histograms, not the
/// constant, and was deliberately not chased. Before that: 0.16 at first, raised to 0.22 as
/// too dark. Each doubling is one stop.
///
/// ⚠ Still a **tuned** value rather than a measured one: it is judged by looking at photographs,
/// which is what `auto_tone_probe` exists to produce.
pub const MEDIAN_TARGET: f32 = 0.20;
/// Where the 99.9th percentile lands after the whites solve.
pub const WHITE_TARGET: f32 = 0.98;
/// Where the 0.1th percentile lands after the blacks solve.
pub const BLACK_TARGET: f32 = 0.01;
/// The exposure the auto may apply, in stops either way.
pub const EXPOSURE_LIMIT: f32 = 2.5;
/// The percentile taken as the black point.
pub const P_LO: f64 = 0.001;
/// The percentile taken as the white point.
pub const P_HI: f64 = 0.999;
/// A sorted sample of this many luminances is plenty; a 26 Mpx frame is never sorted whole.
pub const MAX_SAMPLES: usize = 250_000;

/// ⚠ **The "nothing to clip" guard.** A tail that is not distinct from the body of the
/// histogram is not a black or white point: a slider engages only when its percentile sits at
/// least this factor (one stop) from the median. Every real photograph clears it by a wide
/// margin — a 0.1th percentile is several stops under the median — so it only ever bites on
/// degenerate input, where a bare boundary clamp would have the blacks solve drag the whole
/// frame down to chase a target it cannot reach, fighting the exposure just set.
pub const TAIL_SEPARATION: f32 = 2.0;

/// The three values the auto sets. Everything else in `Edits` is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AutoTone {
    pub exposure: f32,
    pub whites: f32,
    pub blacks: f32,
}

/// The three luminance percentiles the solve reads: 0.1 %, 50 %, 99.9 %.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub lo: f32,
    pub median: f32,
    pub hi: f32,
}

/// The percentiles of luminance over `crop`, or `None` when nothing is inside it.
///
/// ⚠ **Rotation is ignored.** `crop` is taken on the frame as given — the straighten angle, lens
/// and perspective corrections all move pixels by amounts that do not change a percentile
/// appreciably, and mapping through them would need the whole geometry stage for a number
/// that is going to be rounded to a slider anyway. (⚠ The quarter-turn *orientation* is a
/// different matter — it swaps axes — and the caller is expected to hand a crop already in this
/// buffer's frame; the app's `auto_tone_tool` does exactly that.)
///
/// ⚠ Non-finite samples are dropped, not clamped: a `NaN` sorts nowhere and would otherwise
/// land at whichever end `total_cmp` puts it, silently becoming the black or white point.
pub fn percentiles(img: &ImageBuf, crop: Option<CropRect>) -> Option<Percentiles> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let c = crop.unwrap_or_default();
    let x0 = ((c.left.clamp(0.0, 1.0) * w as f32).floor() as usize).min(w);
    let x1 = ((c.right.clamp(0.0, 1.0) * w as f32).ceil() as usize).min(w);
    let y0 = ((c.top.clamp(0.0, 1.0) * h as f32).floor() as usize).min(h);
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h as f32).ceil() as usize).min(h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    // A stride on both axes keeps the sample under `MAX_SAMPLES` without favouring any row.
    let region = (x1 - x0) * (y1 - y0);
    let stride = ((region as f64 / MAX_SAMPLES as f64).sqrt().ceil() as usize).max(1);
    let mut lum: Vec<f32> = Vec::with_capacity(region / (stride * stride) + 1);
    for y in (y0..y1).step_by(stride) {
        for x in (x0..x1).step_by(stride) {
            let l = color::luminance(img.pixel(x, y));
            if l.is_finite() {
                lum.push(l);
            }
        }
    }
    if lum.is_empty() {
        return None;
    }
    lum.sort_unstable_by(f32::total_cmp);
    let at = |q: f64| lum[((lum.len() - 1) as f64 * q).round() as usize];
    Some(Percentiles {
        lo: at(P_LO),
        median: at(0.5),
        hi: at(P_HI),
    })
}

/// The values that put `p`'s median, white point and black point on their targets.
///
/// ⚠⚠ **`median_target` is a parameter because the display rendering also places the level.**
/// The rendering lifts the scene by a baseline exposure before its curve, so a solve that still
/// aimed the *scene* median at [`MEDIAN_TARGET`] would stack the two and open every photograph
/// about a stop too bright — measured at −1.00 EV against the camera's own JPEG. The caller
/// divides the target by the rendering's gain, which is what makes the two compose instead of
/// fighting. See [`median_target_for`].
///
/// Exposure first, from the median; then whites and blacks each by bisection against the real
/// stage with the other at zero (their interaction is small — the two weights never overlap —
/// and accepted). ⚠ Guarded so a black frame gives exposure 0, never `inf`.
pub fn solve(p: Percentiles, median_target: f32) -> AutoTone {
    let exposure = if p.median > 0.0 && p.median.is_finite() {
        (median_target / p.median)
            .log2()
            .clamp(-EXPOSURE_LIMIT, EXPOSURE_LIMIT)
    } else {
        0.0
    };
    let whites = if p.hi > p.median * TAIL_SEPARATION {
        bisect(|w| mapped(p.hi, exposure, w, 0.0), WHITE_TARGET)
    } else {
        0.0
    };
    let blacks = if p.lo * TAIL_SEPARATION < p.median {
        bisect(|b| mapped(p.lo, exposure, 0.0, b), BLACK_TARGET)
    } else {
        0.0
    };
    AutoTone {
        exposure,
        whites,
        blacks,
    }
}

/// [`percentiles`] then [`solve`]; an empty region gives the neutral answer.
pub fn analyse(img: &ImageBuf, crop: Option<CropRect>, median_target: f32) -> AutoTone {
    percentiles(img, crop)
        .map(|p| solve(p, median_target))
        .unwrap_or_default()
}

/// The median target to solve against, given the rendering that will follow.
///
/// ⚠ One function so the two callers — the panel's Auto button and its readout — cannot disagree
/// about where the level is being aimed. With no rendering it is [`MEDIAN_TARGET`] unchanged, so
/// the CLI and every existing test are untouched.
pub fn median_target_for(rendering: edit_model::Rendering, baseline_ev: Option<f32>) -> f32 {
    match rendering {
        edit_model::Rendering::None => MEDIAN_TARGET,
        // ⚠ Divided, not subtracted: the rendering multiplies the scene by `2^ev` *before* its
        // curve, so aiming the scene median at `target / gain` is what lands the **rendered**
        // median where `MEDIAN_TARGET` used to put it. The curve is near-identity around grey,
        // which is what makes that exact rather than approximate.
        edit_model::Rendering::Camera => {
            let gain = baseline_ev
                .unwrap_or(crate::stages::display_rendering::BASELINE_EV)
                .exp2();
            if gain.is_finite() && gain > 0.0 {
                MEDIAN_TARGET / gain
            } else {
                MEDIAN_TARGET
            }
        }
    }
}

/// The luminance a pixel of luminance `l` has after exposure and the tone-regions stage.
///
/// ⚠ Through the **real stages**, on a neutral 1×1 stand-in. That is exact, not approximate:
/// `ToneRegionsStage` scales RGB by a scalar chosen from luminance alone, so every pixel of
/// luminance `l` — whatever its colour — comes out at `l · gain(l)`, and a grey of the same
/// luminance sees the identical gain.
fn mapped(l: f32, exposure: f32, whites: f32, blacks: f32) -> f32 {
    let mut img = ImageBuf::new(1, 1);
    img.set_pixel(0, 0, [l, l, l]);
    let img = ExposureStage::new(exposure).apply(img);
    let img = ToneRegionsStage::new(0.0, 0.0, whites, blacks).apply(img);
    color::luminance(img.pixel(0, 0))
}

/// The slider value in `[-100, 100]` at which the monotone `f` crosses `target`.
///
/// ⚠ Two edges, decided rather than left to fall out of the loop. **No leverage** — the stage
/// cannot move this percentile at all — returns 0: a whites of +100 that changes nothing would
/// sit in the panel as a lie. **Out of range** — the slider has leverage but not enough —
/// returns the boundary, which is as far toward the target as the slider goes: levels-style,
/// and what "clamped to the sliders' own ranges" means.
fn bisect(f: impl Fn(f32) -> f32, target: f32) -> f32 {
    let (mut lo, mut hi) = (-100.0f32, 100.0f32);
    let (f_lo, f_hi) = (f(lo), f(hi));
    if (f_hi - f_lo).abs() < 1e-7 {
        return 0.0;
    }
    if f_lo >= target {
        return lo;
    }
    if f_hi <= target {
        return hi;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (0.5 * (lo + hi)).clamp(-100.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(w: usize, h: usize, v: f32) -> ImageBuf {
        ImageBuf::from_vec(w, h, vec![v; w * h * 3])
    }

    fn lum_percentiles(img: &ImageBuf) -> Percentiles {
        percentiles(img, None).expect("a non-empty image")
    }

    /// Spec test 1. ⚠ Predicted blind spot, and it is real: a uniform grey cannot see a wrong
    /// percentile, because every percentile is the same number. Test 2 is what can.
    #[test]
    fn a_uniform_grey_sets_exposure_from_the_median_and_clips_nothing() {
        for g in [0.05f32, 0.16, 0.4] {
            let got = analyse(&uniform(40, 30, g), None, MEDIAN_TARGET);
            let want = (MEDIAN_TARGET / g).log2();
            assert!(
                (got.exposure - want).abs() < 1e-5,
                "grey {g}: exposure {} vs {want}",
                got.exposure
            );
            assert_eq!(got.whites, 0.0, "grey {g}: nothing to clip at the top");
            assert_eq!(got.blacks, 0.0, "grey {g}: nothing to clip at the bottom");
        }
    }

    /// Spec test 2, the load-bearing one: the only test that can see a bisection against the
    /// wrong stage or a target typo. The pinned tails are chosen inside the window the stage
    /// can actually reach (see the plan), so the targets are hit rather than clamped at.
    #[test]
    fn outliers_land_on_the_targets_through_the_real_stage() {
        let (w, h) = (500, 200);
        let n = w * h;
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            let v = 0.05 + 0.25 * (i as f32 / (n - 1) as f32);
            data.extend_from_slice(&[v, v, v]);
        }
        // 0.2 % pinned dark and 0.2 % pinned bright, scattered so no stride can miss them.
        //
        // ⚠⚠ The bright pin lands at ~0.85 AFTER exposure, INSIDE the whites ramp (2/3..1),
        // and that placement is the whole discriminating power of this test. A first version
        // pinned it at 1.3, above 1.0 — where the stage clamps `l` and its whites weight is
        // exactly 1, so a bisection against a weightless formula agreed with the real stage
        // and the sabotage passed. At 0.85 the weight is 0.55 and the formula is 45 % off.
        let pin = n / 500;
        for k in 0..pin {
            let dark = (k * 997) % n;
            let bright = (k * 991 + 13) % n;
            for c in 0..3 {
                data[dark * 3 + c] = 0.014;
                data[bright * 3 + c] = 0.93;
            }
        }
        let img = ImageBuf::from_vec(w, h, data);
        let before = lum_percentiles(&img);
        let got = analyse(&img, None, MEDIAN_TARGET);

        let out = ExposureStage::new(got.exposure).apply(img);
        let out = ToneRegionsStage::new(0.0, 0.0, got.whites, got.blacks).apply(out);
        let after = lum_percentiles(&out);
        assert!(
            (after.hi - WHITE_TARGET).abs() < 0.01,
            "p_hi {} -> {} (whites {})",
            before.hi,
            after.hi,
            got.whites
        );
        assert!(
            (after.lo - BLACK_TARGET).abs() < 0.002,
            "p_lo {} -> {} (blacks {})",
            before.lo,
            after.lo,
            got.blacks
        );
        assert!(
            got.whites.abs() < 99.0 && got.blacks.abs() < 99.0,
            "the fixture must be solvable, not clamped: {got:?}"
        );
    }

    /// Spec test 3: the premise bisection rests on, asserted rather than assumed. Swept over
    /// luminances on both sides of every weight's knee and above 1.0, where the stage clamps.
    #[test]
    fn the_stage_is_monotone_in_whites_and_in_blacks() {
        let lums = [
            0.0, 0.005, 0.02, 0.1, 0.3, 0.34, 0.5, 0.66, 0.7, 0.9, 1.0, 1.5, 3.0,
        ];
        for l in lums {
            let mut prev_w = f32::NEG_INFINITY;
            let mut prev_b = f32::NEG_INFINITY;
            for s in (-100..=100).step_by(5) {
                let s = s as f32;
                let w = mapped(l, 0.0, s, 0.0);
                let b = mapped(l, 0.0, 0.0, s);
                assert!(w >= prev_w, "whites not monotone at l={l}, s={s}");
                assert!(b >= prev_b, "blacks not monotone at l={l}, s={s}");
                prev_w = w;
                prev_b = b;
            }
        }
    }

    /// Spec test 4: a bright border outside the crop must not move the result; with the crop
    /// removed it must.
    #[test]
    fn the_crop_restricts_the_analysis() {
        let mut img = uniform(100, 100, 0.1);
        for y in 0..100 {
            for x in 0..100 {
                if !(20..80).contains(&x) || !(20..80).contains(&y) {
                    img.set_pixel(x, y, [2.0, 2.0, 2.0]);
                }
            }
        }
        let crop = CropRect {
            left: 0.2,
            top: 0.2,
            right: 0.8,
            bottom: 0.8,
        };
        let inside = percentiles(&img, Some(crop)).unwrap();
        assert_eq!(inside.median, 0.1);
        assert_eq!(inside.hi, 0.1, "the border leaked into the crop");
        let whole = percentiles(&img, None).unwrap();
        assert_eq!(whole.hi, 2.0, "with no crop the border is the white point");
        assert_eq!(whole.median, 2.0, "64 % of the frame is border");
    }

    /// Spec test 5: no panic, finite outputs inside the slider ranges.
    #[test]
    fn degenerate_inputs_give_finite_values_inside_the_ranges() {
        let mut nan = uniform(10, 10, 0.2);
        nan.set_pixel(3, 3, [f32::NAN, f32::NAN, f32::NAN]);
        nan.set_pixel(4, 4, [f32::INFINITY, 0.0, 0.0]);
        let cases = [
            ("black", uniform(10, 10, 0.0)),
            ("1x1", uniform(1, 1, 0.3)),
            ("nan", nan),
            ("empty", ImageBuf::new(0, 0)),
            ("all nan", uniform(4, 4, f32::NAN)),
        ];
        for (name, img) in cases {
            let got = analyse(&img, None, MEDIAN_TARGET);
            assert!(
                got.exposure.is_finite(),
                "{name}: exposure {}",
                got.exposure
            );
            assert!(got.exposure.abs() <= EXPOSURE_LIMIT, "{name}: {got:?}");
            assert!(
                got.whites.is_finite() && got.whites.abs() <= 100.0,
                "{name}: {got:?}"
            );
            assert!(
                got.blacks.is_finite() && got.blacks.abs() <= 100.0,
                "{name}: {got:?}"
            );
        }
        assert_eq!(
            analyse(&uniform(10, 10, 0.0), None, MEDIAN_TARGET).exposure,
            0.0
        );
        // ⚠ The discriminating half. Asserting only on `analyse`'s output cannot see the
        // `is_finite` guard: a NaN percentile merely fails the tail-separation check and yields
        // the same 0 the correct code does. The percentiles themselves must be finite, and a
        // frame with no finite sample at all must say so rather than hand back NaN.
        let mut nan = uniform(10, 10, 0.2);
        nan.set_pixel(3, 3, [f32::NAN, f32::NAN, f32::NAN]);
        nan.set_pixel(4, 4, [f32::INFINITY, 0.0, 0.0]);
        let p = percentiles(&nan, None).expect("98 finite samples remain");
        assert!(
            p.lo.is_finite() && p.median.is_finite() && p.hi.is_finite(),
            "a non-finite sample leaked into the percentiles: {p:?}"
        );
        assert_eq!((p.lo, p.hi), (0.2, 0.2), "the finite samples are all 0.2");
        assert!(percentiles(&uniform(4, 4, f32::NAN), None).is_none());
        // A crop that encloses nothing is the neutral answer, not a panic.
        let none = CropRect {
            left: 0.5,
            top: 0.5,
            right: 0.5,
            bottom: 0.5,
        };
        assert_eq!(
            analyse(&uniform(10, 10, 0.2), Some(none), MEDIAN_TARGET),
            AutoTone::default()
        );
    }

    /// ⚠⚠ **Auto Tone and the display rendering both place the level, and this is the only test
    /// that can see them stack.** With the rendering on, the scene is lifted by a baseline
    /// exposure *before* the curve, so a solve still aiming the scene median at
    /// [`MEDIAN_TARGET`] opens every photograph about a stop too bright — measured at −1.00 EV
    /// against the camera's own JPEG before this was threaded through. Dividing the target by
    /// the gain is what makes the two compose.
    #[test]
    fn the_median_target_is_divided_by_the_renderings_gain() {
        use edit_model::Rendering;
        // No rendering: unchanged, so the CLI and every existing test are untouched.
        assert_eq!(median_target_for(Rendering::None, None), MEDIAN_TARGET);
        assert_eq!(median_target_for(Rendering::None, Some(2.0)), MEDIAN_TARGET);

        // ⚠ Divided, not multiplied: a solve that *raised* the target would open a photograph
        // two stops bright rather than one, and "it changed" alone cannot tell the two apart.
        let one_stop = median_target_for(Rendering::Camera, Some(1.0));
        assert!(
            (one_stop - MEDIAN_TARGET / 2.0).abs() < 1e-6,
            "a +1 EV baseline did not halve the target: {one_stop}"
        );

        // ⚠ And an absent baseline falls back to the renderer's own constant, so a profile that
        // declares none still composes rather than stacking.
        let fallback = median_target_for(Rendering::Camera, None);
        assert!(
            fallback < MEDIAN_TARGET,
            "an absent baseline left the target unchanged: {fallback}"
        );
    }

    /// ⚠ A degenerate baseline must not make the target zero or infinite — the solve would then
    /// return the exposure clamp on every photograph.
    #[test]
    fn a_degenerate_baseline_leaves_the_target_usable() {
        use edit_model::Rendering;
        for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30] {
            let t = median_target_for(Rendering::Camera, Some(bad));
            assert!(t.is_finite() && t > 0.0, "{bad} gave {t}");
        }
    }

    /// Spec test 6, both directions. ⚠ The spec's wording ("a frame 6 EV under lands at −2.5")
    /// has the sign backwards: an *under*-exposed frame wants a positive correction. Both ends
    /// are asserted so the clamp cannot be one-sided.
    #[test]
    fn exposure_clamps_at_two_and_a_half_stops() {
        let under = analyse(&uniform(20, 20, MEDIAN_TARGET / 64.0), None, MEDIAN_TARGET);
        assert_eq!(under.exposure, EXPOSURE_LIMIT);
        let over = analyse(&uniform(20, 20, MEDIAN_TARGET * 64.0), None, MEDIAN_TARGET);
        assert_eq!(over.exposure, -EXPOSURE_LIMIT);
    }

    /// The no-leverage edge: whites has no effect below 2/3, so a white point that lands at
    /// 0.5 after exposure gets 0, not +100 — a slider at +100 that moves nothing is a lie.
    #[test]
    fn a_percentile_the_slider_can_now_move_is_solved_rather_than_zeroed() {
        // ⚠⚠ **This test used to assert the opposite, and the branch it covered is now inert.**
        // It was written when the Whites weight was `max(0, 3l − 2)` on *linear* luminance, which
        // is exactly zero below linear 2/3 — so a highlight at 0.5 could not be moved at all and
        // the solver's no-leverage rule returned 0. With the weight a cube of the *encoded*
        // luminance the slider always has leverage, so that case no longer exists for any input
        // a photograph can produce.
        //
        // ⚠ The guard itself is **kept as defence in depth and is recorded as unreachable**
        // rather than deleted or covered by an invented fixture: `white_w` is zero only at `l = 0`,
        // which the `hi > 2·median` guard already excludes. Inventing a test that appeared to
        // cover it would be worse than saying so.
        let got = solve(
            Percentiles {
                lo: 0.012,
                median: MEDIAN_TARGET,
                hi: 0.5,
            },
            MEDIAN_TARGET,
        );
        assert_eq!(got.exposure, 0.0);
        assert!(
            got.whites.abs() > 1.0,
            "a highlight at 0.5 must now be solvable, got {}",
            got.whites
        );
        assert!(got.blacks.abs() < 99.0, "the blacks half is unaffected");
    }

    /// The out-of-range edge: leverage but not enough reaches the boundary — as far toward the
    /// target as the slider goes, not 0 and not something in between.
    #[test]
    fn the_solve_reaches_the_boundary_when_the_target_is_out_of_range() {
        let got = solve(
            Percentiles {
                lo: 0.05,
                median: MEDIAN_TARGET,
                // ⚠ 10.0, not the 3.0 this test shipped with: the end pair got stronger when its
                // weights moved to a cube on encoded luminance with its own `END_STRENGTH`, and
                // 0.98 from 3.0 became *reachable* — which quietly turned this into a test of the
                // solved path rather than of the boundary rule. A fixture the sliders genuinely
                // cannot satisfy is what keeps it discriminating.
                hi: 10.0,
            },
            MEDIAN_TARGET,
        );
        assert_eq!(
            got.blacks, -100.0,
            "0.05 cannot reach 0.01: crush as far as it goes"
        );
        assert_eq!(
            got.whites, -100.0,
            "10.0 cannot reach 0.98: pull down as far as it goes"
        );
        let got = solve(
            Percentiles {
                lo: 0.001,
                median: MEDIAN_TARGET,
                // ⚠ 0.5 for the same reason as the 10.0 above — the stronger end pair brought 0.98
                // within reach from 0.7, so that fixture stopped testing the boundary. At 0.5 the
                // most Whites can do is 0.64.
                hi: 0.5,
            },
            MEDIAN_TARGET,
        );
        assert_eq!(got.blacks, 100.0, "0.001 wants lifting as far as it goes");
        assert_eq!(
            got.whites, 100.0,
            "0.5 cannot reach 0.98: lift as far as it goes"
        );
    }

    /// ⚠ The guard that spec test 1 forced: a tail one stop or less from the median is not a
    /// black or white point. Both sides, and the discriminating half — a tail *just* past one
    /// stop does engage.
    #[test]
    fn a_tail_not_distinct_from_the_body_is_not_clipped() {
        let m = MEDIAN_TARGET;
        let near = solve(
            Percentiles {
                lo: m / 1.9,
                median: m,
                hi: m * 1.9,
            },
            MEDIAN_TARGET,
        );
        assert_eq!((near.whites, near.blacks), (0.0, 0.0));
        let far = solve(
            Percentiles {
                lo: m / 2.1,
                median: m,
                hi: m * 2.1,
            },
            MEDIAN_TARGET,
        );
        assert_ne!(
            far.blacks, 0.0,
            "a tail a stop under the median is a black point"
        );
        // hi = 0.336 has no whites leverage, so the guard's other half is seen through blacks
        // alone here and through the whites boundary below.
        let far_hi = solve(
            Percentiles {
                lo: m / 2.1,
                median: m,
                hi: 0.8,
            },
            MEDIAN_TARGET,
        );
        assert_ne!(far_hi.whites, 0.0, "a tail above the body is a white point");
    }

    /// The sample cap holds and the sub-sampled percentiles stay faithful.
    #[test]
    fn a_large_frame_is_subsampled_without_moving_the_percentiles() {
        let (w, h) = (1200, 800);
        let n = w * h;
        assert!(n > MAX_SAMPLES);
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            // ⚠ A vertical gradient, deliberately: a sample that took the first `MAX_SAMPLES`
            // pixels row-major would see only the dark rows and put the median near 0.13. A
            // pseudo-random pattern could not tell that sabotage from a correct stride.
            let v = (i / w) as f32 / (h - 1) as f32;
            data.extend_from_slice(&[v, v, v]);
        }
        let p = percentiles(&ImageBuf::from_vec(w, h, data), None).unwrap();
        assert!((p.median - 0.5).abs() < 0.01, "median {}", p.median);
        assert!((p.lo - 0.001).abs() < 0.003, "lo {}", p.lo);
        assert!((p.hi - 0.999).abs() < 0.003, "hi {}", p.hi);
    }
}
