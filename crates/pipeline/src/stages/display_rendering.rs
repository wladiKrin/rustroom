//! Scene-referred light onto a display.
//!
//! ⚠⚠ **A sigmoid in log-exposure, and the measurement is why.** The camera's own transfer was
//! measured by pairing a scene-linear decode against the embedded JPEG of the same frame
//! (`curve_probe`, 7 X-T4 frames pooled). Anchored at middle grey it **crushes** below (0.02× at
//! −4 EV, 0.28× at −2.25) and **rolls off** above (0.79× at +2.25), with the ratio peaking at 1.0
//! *at grey*. That non-monotone ratio is the whole shape, and it rules out the compressive family
//! this was going to use: `x^c/(x^c + k)` and every power curve have a *monotone* log-log slope,
//! so anchored at grey they can crush the toe or roll the shoulder, never both.
//!
//! ⚠⚠ **A FIRST MEASUREMENT SAID THE CAMERA *LIFTS* THE MIDTONES 1.5–2.3×, AND THAT WAS THE
//! METERING, NOT THE CURVE.** It pooled frames raw. Each frame's JPEG carries that frame's own
//! exposure decision, and across these 7 frames the camera metered up by **+0.88 to +1.57 EV** —
//! which is exactly the lift the first pass attributed to the transfer. Rescaling each frame so
//! its own grey point lands on [`GREY`] removes it, and what is left never exceeds 1.0×. **The
//! 0.69 EV spread between frames is also the evidence for the split below**: no fixed curve can
//! encode a decision that moves 0.69 EV from frame to frame.
//!
//! ⚠⚠ **The stage owns the SHAPE and not the LEVEL.** A camera's JPEG brightness includes its own
//! per-frame metering decision, which no fixed curve can encode — that is Auto Tone's job
//! (M5.4a). Chasing it here would bake one frame's metering into every photograph, which is why
//! middle grey is a fixed point and the acceptance test asks for a *constant* ratio against the
//! camera rather than a zero one.

use crate::spline::Spline;
use crate::PipelineStage;
use core_image::ImageBuf;

/// Middle grey in the scene-referred working space.
const GREY: f32 = 0.18;

/// The log-exposure window the curve is defined over, in stops relative to [`GREY`].
///
/// ⚠ Everything below `T_MIN` renders black and everything above `T_MAX` renders white, so the
/// window has to cover more dynamic range than any photograph carries. 13 stops does.
const T_MIN: f32 = -8.0;
const T_MAX: f32 = 5.0;

/// The baseline exposure to assume when a profile declares none, in stops.
///
/// ⚠⚠ **This is the missing step between "sensor readings" and "a picture", not a fudge.** A RAW
/// is metered to leave headroom above the subject so highlights do not clip, and libraw's
/// `no_auto_bright` applies no normalisation at all — so 1.0 is sensor saturation and the
/// subject's midtone lands well below [`GREY`]. Measured on 7 X-T4 frames (`curve_probe`), the
/// scene value the camera renders as middle grey is **0.061–0.098**, i.e. 0.88–1.57 stops low;
/// the median is 2.185× = +1.13 EV.
///
/// ⚠ **X-T4-derived, like [`BUILT_IN`], and for the same reason: it is the fallback.** The right
/// source is the profile's own `BaselineExposure` (DNG tag 50730), which is read when present —
/// but it is optional in the spec and most community profiles omit it, including the X-T4's.
/// Manufacturers differ in how much headroom their metering leaves, so on another body this is a
/// reasonable guess rather than that camera's number.
///
/// ⚠ It applies whichever curve follows, including a profile's, because it describes the
/// **sensor's levels** and not the look. The two fall back independently.
pub const BASELINE_EV: f32 = 1.13;

/// ⚠⚠ **4096, and the number is measured rather than chosen.** The table is linear in *scene*
/// value while the curve is defined in log-exposure, so its resolution is worst exactly where the
/// eye is most sensitive — the shadows. At 1024 entries it drifts **0.61 of an 8-bit level** from
/// direct evaluation of the spline; interpolation error falls with the square of the step, so
/// 4096 brings that to well under a tenth of a level. The cost is 16 KB per stage.
/// `the_table_agrees_with_direct_evaluation_below_half_an_eight_bit_level` is the guard.
const LUT_SIZE: usize = 4096;

/// The largest scene value the LUT resolves *before* the baseline gain; above it the curve is
/// flat at 1 anyway.
///
/// ⚠ The instance divides this by its gain, so the table always covers exactly the scene range
/// up to white rather than spending half its entries on values the curve has already flattened.
const LUT_TOP: f32 = GREY * 32.0;

/// The built-in filmic curve, as `(u, y)` where `u` is the normalised log-exposure
/// `(t - T_MIN) / (T_MAX - T_MIN)` and `y` is display-linear output.
///
/// ⚠⚠ **Normalised into `[0, 1]` because [`Spline`] clamps its control points there.** Feeding it
/// raw log-exposure would collapse every point onto the edges and flatten the curve to garbage.
///
/// ⚠ **Measured, not chosen**, from 7 pooled X-T4 frames (`curve_probe`) — every point up to
/// `0.750` is the median the camera actually produced at that exposure.
///
/// ⚠⚠ **THE TOP THREE BINS ARE DISCARDED AND THE SHOULDER IS CONSTRUCTED, because the
/// measurement is biased exactly there.** A bin above ~+2 EV holds a growing fraction of pixels
/// the JPEG has *clipped* to 255, which drags its median up — so the raw data appears to
/// **steepen** toward white (slopes 3.2, 5.2, 7.0 across the last three bins), which no tone
/// curve does. Read literally it would have given the rendering a contrast bump in the
/// highlights. The three points above `0.750` instead continue the measured slope and roll it
/// off monotonically to white.
///
/// ⚠ Consequence, stated rather than discovered: **our shoulder is gentler than the camera's.**
/// At +2.75 EV the camera reads 0.95 (i.e. essentially clipped) where this gives ~0.68. A
/// photograph opens with more highlight roll-off than its JPEG — deliberate for an editor, and
/// the one place this rendering does not match the camera.
const BUILT_IN: [[f32; 2]; 12] = [
    [0.000, 0.0000],
    [0.250, 0.0002], // −4.75 EV: the camera reads black from here down
    [0.327, 0.0015],
    [0.404, 0.0054],
    [0.481, 0.0264],
    [0.558, 0.0839],
    [0.615, 0.1800], // middle grey, the fixed point Auto Tone's level sits around
    [0.673, 0.2887],
    [0.750, 0.4793], // +1.75 EV: the last bin the clipping bias leaves trustworthy
    [0.820, 0.6600],
    [0.900, 0.8300],
    [1.000, 1.0000],
];

pub struct DisplayRenderingStage {
    lut: Vec<f32>,
    /// The largest scene value the table resolves — `LUT_TOP` divided by the baseline gain.
    lut_max: f32,
}

impl DisplayRenderingStage {
    /// The rendering: a curve, and the baseline exposure that places the scene under it.
    ///
    /// `curve` is a profile's `ProfileToneCurve` when it has a usable one, else `None` for the
    /// built-in. `baseline_ev` is the profile's `BaselineExposure`, or `None` to fall back to
    /// [`BASELINE_EV`].
    ///
    /// ⚠ A profile curve is defined on `[0, 1]` of *linear* input and its input is therefore
    /// **clamped**, so scene values above 1.0 render white. The built-in curve does not clamp —
    /// it is total on `[0, ∞)`. That difference is a caveat of using a profile's curve, not a
    /// bug, and it has its own test.
    pub fn new(curve: Option<&[[f32; 2]]>, baseline_ev: Option<f32>) -> Self {
        let gain = baseline_ev.unwrap_or(BASELINE_EV).exp2();
        match curve {
            Some(p) => {
                let spline = Spline::new(p);
                Self::build(gain, move |x| spline.eval(x.clamp(0.0, 1.0)))
            }
            None => {
                let spline = Spline::new(&BUILT_IN);
                Self::build(gain, move |x| spline.eval(normalised_log(x)))
            }
        }
    }

    fn build(gain: f32, f: impl Fn(f32) -> f32) -> Self {
        // ⚠ A non-finite or non-positive gain would make every LUT entry garbage; repairing it to
        // 1.0 keeps a malformed profile rendering rather than blacking the image out.
        let gain = if gain.is_finite() && gain > 0.0 {
            gain
        } else {
            1.0
        };
        let lut_max = LUT_TOP / gain;
        let lut = (0..LUT_SIZE)
            .map(|i| {
                let x = i as f32 / (LUT_SIZE - 1) as f32 * lut_max;
                // ⚠⚠ The gain multiplies the scene value going *into* the curve — it is a change
                // of exposure, not of the transfer — so it costs nothing per pixel and vanishes
                // entirely when the rendering is off.
                f(x * gain).clamp(0.0, 1.0)
            })
            .collect();
        Self { lut, lut_max }
    }

    /// The curve's output for one scene value.
    ///
    /// ⚠ Total on `[0, ∞)`: a non-finite or negative input reads as 0, and anything past the
    /// table's top takes the last entry, which the curve has already flattened to 1.
    pub fn map(&self, x: f32) -> f32 {
        if !x.is_finite() || x <= 0.0 {
            return self.lut[0];
        }
        let pos = (x / self.lut_max).min(1.0) * (LUT_SIZE - 1) as f32;
        let i = pos.floor() as usize;
        if i >= LUT_SIZE - 1 {
            return self.lut[LUT_SIZE - 1];
        }
        let frac = pos - i as f32;
        self.lut[i] + frac * (self.lut[i + 1] - self.lut[i])
    }
}

/// Scene value to normalised log-exposure, clamped into the spline's domain.
fn normalised_log(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let t = (x / GREY).log2();
    ((t - T_MIN) / (T_MAX - T_MIN)).clamp(0.0, 1.0)
}

impl PipelineStage for DisplayRenderingStage {
    fn name(&self) -> &'static str {
        "display_rendering"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠⚠ **Per channel, not on luminance with a scalar gain.** Every other tonal stage here
        // preserves hue deliberately; this one must not. Curving the channels independently is
        // where a camera rendering's shadow saturation and its roll toward white come from, and
        // it is what the DNG spec does with `ProfileToneCurve`. A neutral grey still stays
        // exactly neutral, because all three channels take the same curve.
        image.map_pixels(|[r, g, b]| [self.map(r), self.map(g), self.map(b)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ⚠ Every test below passes `Some(0.0)` for the baseline: they measure the **transfer**, and
    // a baseline shift would move every probe point along it. The baseline has its own tests.

    /// ⚠ Total and monotone over everything a scene-referred buffer can hold, including values
    /// far above 1.0 and the degenerate ones.
    #[test]
    fn the_curve_is_total_and_monotone() {
        let s = DisplayRenderingStage::new(None, Some(0.0));
        let mut last = -1.0;
        for i in 0..=2000 {
            let x = i as f32 / 2000.0 * 64.0;
            let y = s.map(x);
            assert!((0.0..=1.0).contains(&y), "x={x} left the range: {y}");
            assert!(y >= last - 1e-6, "x={x} went backwards: {y} after {last}");
            last = y;
        }
        for x in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::MAX] {
            let y = s.map(x);
            assert!(y.is_finite() && (0.0..=1.0).contains(&y), "{x} gave {y}");
        }
    }

    /// ⚠⚠ **The shape, and the only test here that can see a curve built in linear instead of log
    /// space, fitted with the wrong sign, or replaced by any power curve.** Total-and-monotone
    /// passes against a plain ramp; this does not.
    ///
    /// The discriminating property is that the **ratio is non-monotone, peaking at grey**: it
    /// crushes below and rolls off above. Anchored at grey, `y = GREY·(x/GREY)^c` has ratio
    /// `(x/GREY)^(c−1)`, which is monotone for every `c` — so no power curve, and no plain gain,
    /// can pass this.
    #[test]
    fn the_ratio_peaks_at_grey_and_falls_away_on_both_sides() {
        let s = DisplayRenderingStage::new(None, Some(0.0));
        let ratio = |x: f32| s.map(x) / x;
        let (deep, grey, high) = (ratio(0.005), ratio(GREY), ratio(2.0));
        assert!(deep < 0.2, "the toe does not crush: {deep}");
        assert!((grey - 1.0).abs() < 0.06, "grey is not near unity: {grey}");
        assert!(high < 0.7, "there is no shoulder: {high}");
        assert!(
            grey > 2.0 * deep && grey > 1.5 * high,
            "the ratio is monotone, so this is a power curve: {deep} / {grey} / {high}"
        );
    }

    /// ⚠ Middle grey is the anchor the whole design hangs on — the rendering owns the shape and
    /// Auto Tone owns the level, which only works if grey is a fixed point.
    #[test]
    fn middle_grey_is_preserved() {
        let s = DisplayRenderingStage::new(None, Some(0.0));
        assert!(
            (s.map(GREY) - GREY).abs() < 0.01,
            "grey moved to {}",
            s.map(GREY)
        );
    }

    /// ⚠⚠ **Per channel, and both halves matter.** A neutral-only assertion passes against a
    /// luminance-and-scalar-gain model, which is what every other stage here uses.
    #[test]
    fn a_neutral_stays_neutral_and_a_colour_shifts() {
        let s = DisplayRenderingStage::new(None, Some(0.0));
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.2, 0.2, 0.2]);
        img.set_pixel(1, 0, [0.30, 0.10, 0.02]);
        let out = s.apply(img);

        let n = out.pixel(0, 0);
        assert!(
            (n[0] - n[1]).abs() < 1e-6 && (n[1] - n[2]).abs() < 1e-6,
            "grey shifted: {n:?}"
        );

        let c = out.pixel(1, 0);
        let before = 0.30 / 0.10;
        let after = c[0] / c[1];
        assert!(
            (after - before).abs() > 0.05,
            "the channel ratio did not change, so this is a luminance model: {before} -> {after}"
        );
    }

    /// ⚠⚠ **The baseline shifts the scene under the curve, and this is the only test that can
    /// see it applied in the wrong direction.** A camera meters to leave highlight headroom, so
    /// the subject's midtone sits *below* [`GREY`]; the baseline puts it back. Applied inverted,
    /// a photograph would open two stops darker still — and every shape test above passes either
    /// way, because they all pin the baseline to zero.
    #[test]
    fn the_baseline_lifts_the_scene_under_the_curve() {
        let none = DisplayRenderingStage::new(None, Some(0.0));
        let up = DisplayRenderingStage::new(None, Some(1.0));
        // A scene value one stop below grey renders as grey once lifted by one stop.
        let x = GREY / 2.0;
        assert!(
            up.map(x) > none.map(x),
            "the baseline darkened instead of lifting: {} vs {}",
            up.map(x),
            none.map(x)
        );
        assert!(
            (up.map(x) - none.map(GREY)).abs() < 0.01,
            "+1 EV did not move a -1 EV value onto grey: {} vs {}",
            up.map(x),
            none.map(GREY)
        );
    }

    /// ⚠⚠ **`None` falls back to the measured constant; `Some(0.0)` does not.** A profile that
    /// declares no shift has made a statement and must be honoured, and a test that only ever
    /// passes a number cannot tell the two apart — which is exactly how the fallback would come
    /// to override every profile that says zero.
    #[test]
    fn an_absent_baseline_falls_back_and_a_declared_zero_does_not() {
        let fallback = DisplayRenderingStage::new(None, None);
        let declared = DisplayRenderingStage::new(None, Some(0.0));
        let measured = DisplayRenderingStage::new(None, Some(BASELINE_EV));
        let x = GREY / 2.0;
        assert!(
            (fallback.map(x) - measured.map(x)).abs() < 1e-6,
            "None did not fall back to BASELINE_EV"
        );
        assert!(
            fallback.map(x) > declared.map(x) + 0.02,
            "a declared zero was overridden by the fallback"
        );
    }

    /// A malformed profile must not black the image out: a non-finite or non-positive gain is
    /// repaired to no shift rather than producing a table of garbage.
    #[test]
    fn a_degenerate_baseline_is_repaired_rather_than_fatal() {
        let good = DisplayRenderingStage::new(None, Some(0.0));
        for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30] {
            let s = DisplayRenderingStage::new(None, Some(bad));
            let y = s.map(GREY);
            assert!(y.is_finite() && (0.0..=1.0).contains(&y), "{bad} gave {y}");
        }
        // ⚠ And the repair lands on *no shift*, not on some other arbitrary gain — checked with
        // a value large enough that `exp2` genuinely overflows to infinity.
        let repaired = DisplayRenderingStage::new(None, Some(1e30));
        assert!((repaired.map(GREY) - good.map(GREY)).abs() < 1e-6);
    }

    /// ⚠⚠ **The LUT is linear in scene value while the curve is defined in log-exposure, so its
    /// resolution is worst in the shadows — measured rather than assumed.** The table also
    /// shrinks its top with the baseline gain, which is what keeps that resolution usable
    /// instead of spending entries on values the curve has already flattened to white.
    ///
    /// ⚠ **A tenth of a level, not half of one, and the tightening was forced by a sabotage.**
    /// At half a level the shrink was invisible — removing it still passed — because 4096
    /// entries are enough on their own to clear that bar. Measured at the real baseline, the
    /// shrink is worth **4.6×**: 0.05 of a level against 0.23 without it. The bound is what the
    /// design delivers with 2× margin, not a number chosen to make a red test green.
    #[test]
    fn the_table_agrees_with_direct_evaluation_below_a_tenth_of_an_eight_bit_level() {
        let spline = Spline::new(&BUILT_IN);
        for ev in [0.0, BASELINE_EV] {
            let s = DisplayRenderingStage::new(None, Some(ev));
            let gain = ev.exp2();
            let mut worst = 0.0f32;
            for i in 0..=4000 {
                // Swept in log space, so the shadows get as many probes as the highlights.
                let x = GREY * 2f32.powf(-8.0 + 13.0 * i as f32 / 4000.0);
                let want = spline.eval(normalised_log(x * gain)).clamp(0.0, 1.0);
                // Compared display-encoded, which is where an error is actually visible.
                let e = (color::linear_to_srgb(s.map(x)) - color::linear_to_srgb(want)).abs();
                worst = worst.max(e);
            }
            assert!(
                worst < 1.0 / 2550.0,
                "at {ev} EV the table drifts {:.2} of an 8-bit level from the curve",
                worst * 255.0
            );
        }
    }

    /// ⚠ A profile's curve is defined on `[0, 1]`, so it clamps above 1.0 where the built-in does
    /// not. Asserting both is what keeps the difference deliberate rather than accidental.
    #[test]
    fn a_profile_curve_clamps_above_one_and_the_built_in_does_not() {
        let profile =
            DisplayRenderingStage::new(Some(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]), Some(0.0));
        // ⚠ Compared at 2.0 against 4.0, both well *past* the clamp — not at 1.0, which sits on
        // the boundary where the LUT cell straddles it and interpolation lands a hair short of
        // 1.0. That is resolution, not behaviour, and asserting on it would be a test wrong
        // about correct code.
        assert!(
            (profile.map(4.0) - profile.map(2.0)).abs() < 1e-6,
            "the profile curve did not clamp: {} vs {}",
            profile.map(2.0),
            profile.map(4.0)
        );
        assert!(profile.map(2.0) > 0.99, "it clamped to the wrong value");
        let built_in = DisplayRenderingStage::new(None, Some(0.0));
        assert!(
            built_in.map(4.0) > built_in.map(1.0),
            "the built-in clamped at 1.0"
        );
    }
}
