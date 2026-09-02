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

const LUT_SIZE: usize = 1024;

/// The largest scene value the LUT resolves; above it the curve is flat at 1 anyway.
const LUT_MAX: f32 = GREY * 32.0;

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
}

impl DisplayRenderingStage {
    /// The built-in filmic rendering.
    pub fn built_in() -> Self {
        Self::from_normalised(&BUILT_IN)
    }

    /// A profile's `ProfileToneCurve`, which is defined on `[0, 1]` of *linear* input rather than
    /// on log-exposure.
    ///
    /// ⚠ Its input is **clamped** to `[0, 1]`, so scene values above 1.0 render white. The
    /// built-in curve does not clamp — it is total on `[0, ∞)`. That difference is a caveat of
    /// using a profile's curve, not a bug, and it has its own test.
    pub fn from_profile(points: &[[f32; 2]]) -> Self {
        let spline = Spline::new(points);
        Self {
            lut: Self::build(|x| spline.eval(x.clamp(0.0, 1.0))),
        }
    }

    fn from_normalised(points: &[[f32; 2]]) -> Self {
        let spline = Spline::new(points);
        Self {
            lut: Self::build(|x| spline.eval(normalised_log(x))),
        }
    }

    fn build(f: impl Fn(f32) -> f32) -> Vec<f32> {
        (0..LUT_SIZE)
            .map(|i| {
                let x = i as f32 / (LUT_SIZE - 1) as f32 * LUT_MAX;
                f(x).clamp(0.0, 1.0)
            })
            .collect()
    }

    /// The curve's output for one scene value.
    ///
    /// ⚠ Total on `[0, ∞)`: a non-finite or negative input reads as 0, and anything past
    /// [`LUT_MAX`] takes the last entry, which the curve has already flattened to 1.
    pub fn map(&self, x: f32) -> f32 {
        if !x.is_finite() || x <= 0.0 {
            return self.lut[0];
        }
        let pos = (x / LUT_MAX).min(1.0) * (LUT_SIZE - 1) as f32;
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

    /// ⚠ Total and monotone over everything a scene-referred buffer can hold, including values
    /// far above 1.0 and the degenerate ones.
    #[test]
    fn the_curve_is_total_and_monotone() {
        let s = DisplayRenderingStage::built_in();
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
        let s = DisplayRenderingStage::built_in();
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
        let s = DisplayRenderingStage::built_in();
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
        let s = DisplayRenderingStage::built_in();
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

    /// ⚠ A profile's curve is defined on `[0, 1]`, so it clamps above 1.0 where the built-in does
    /// not. Asserting both is what keeps the difference deliberate rather than accidental.
    #[test]
    fn a_profile_curve_clamps_above_one_and_the_built_in_does_not() {
        let profile = DisplayRenderingStage::from_profile(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
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
        let built_in = DisplayRenderingStage::built_in();
        assert!(
            built_in.map(4.0) > built_in.map(1.0),
            "the built-in clamped at 1.0"
        );
    }
}
