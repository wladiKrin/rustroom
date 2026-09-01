//! Kelvin + tint white balance: the daylight locus, and the gains that reach it.
//!
//! ⚠⚠ **Two directions, and only one existed before this.** [`crate::camera::neutral_temperature`]
//! maps a camera neutral **to** Kelvin — it is what gives a RAW its as-shot reading, and it works
//! from libraw's single matrix by being passed the same one twice, since `blend(m, m, g)` is `m`
//! for any `g`. What a *slider* needs is the inverse, and that is this module. The roadmap once
//! said "the solver already exists"; the readout was free, this was not.
//!
//! ⚠ **The reference for a non-RAW file is its own white point.** A JPEG has already been balanced
//! by the camera, so a neutral object in it reads as equal RGB and its baseline **is** sRGB's D65.
//! The same code serves both; only the matrix differs.

/// D65 — the white point of sRGB and of a file the camera has already balanced.
pub const BASELINE_KELVIN: f32 = 6504.0;

/// Chromaticity of the CIE daylight locus at `kelvin`.
///
/// ⚠ The **daylight** locus, not the Planckian one. They diverge below ~4000 K, and every raw
/// converter's temperature slider follows daylight — matching the Planckian curve instead would put
/// our numbers at odds with every other tool at the tungsten end, which is exactly where a shared
/// vocabulary matters most.
pub fn locus_xy(kelvin: f32) -> (f32, f32) {
    let t = kelvin.clamp(1667.0, 25000.0);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 7000.0 {
        -4.6070e9 / t3 + 2.9678e6 / t2 + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / t3 + 1.9018e6 / t2 + 0.24748e3 / t + 0.237040
    };
    let y = -3.000 * x * x + 2.870 * x - 0.275;
    (x, y)
}

/// How far `tint` moves a chromaticity per unit, perpendicular to the locus.
///
/// ⚠ The one tuning constant here. It sets what ±100 means, chosen so the range covers the casts
/// real light produces — fluorescent green, an overcast magenta — without reaching absurdity.
const TINT_SCALE: f32 = 0.0006;

/// Offset a chromaticity perpendicular to the locus. Positive is magenta, negative green.
pub fn with_tint((x, y): (f32, f32), tint: f32) -> (f32, f32) {
    // The locus is y = -3x² + 2.87x - 0.275, so dy/dx = -6x + 2.87; the perpendicular to
    // (1, dy/dx) is (-dy/dx, 1), normalised.
    let dy_dx = -6.000 * x + 2.870;
    let n = (1.0 + dy_dx * dy_dx).sqrt();
    (
        x + (-dy_dx / n) * tint * TINT_SCALE,
        y + (1.0 / n) * tint * TINT_SCALE,
    )
}

/// Per-channel gains that render an illuminant of `kelvin`/`tint` neutral, for a source whose
/// linear RGB → XYZ matrix is `rgb_to_xyz`.
///
/// ⚠ Luminance-normalised, exactly as the relative model it replaces was: a white-balance change
/// moves colour and not brightness. ⚠ It normalises by [`crate::luminance`], which is **ProPhoto's**
/// Y — the working space's — matching what `WhiteBalanceStage` already did. It is *not* the Rec.709
/// row of `SRGB_TO_XYZ`, and the two must not be confused.
pub fn gains_for(kelvin: f32, tint: f32, rgb_to_xyz: &[[f32; 3]; 3]) -> [f32; 3] {
    let (x, y) = with_tint(locus_xy(kelvin), tint);
    if y.abs() < 1e-6 {
        return [1.0, 1.0, 1.0];
    }
    // The illuminant as XYZ at unit luminance, then into the source's own RGB.
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    let Some(inv) = crate::camera::invert(rgb_to_xyz) else {
        return [1.0, 1.0, 1.0];
    };
    let rgb = crate::spaces::apply(&inv, xyz);
    // ⚠ A channel at or below zero means the illuminant falls outside this source's gamut. Taking
    // its reciprocal would give a **negative gain** and invert that channel across the whole
    // photograph — a failure that looks like a bug in something else entirely.
    let mut gains = [1.0f32; 3];
    for i in 0..3 {
        if rgb[i] > 1e-6 {
            gains[i] = 1.0 / rgb[i];
        }
    }
    let lum = crate::luminance(gains);
    if lum > 1e-6 {
        for c in &mut gains {
            *c /= lum;
        }
    }
    gains
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **Cross-checked against PUBLISHED values, not against ourselves.** A self-consistent pair
    /// of wrong transforms passes a round trip happily, so this is the only test here that can see
    /// a wrong locus. D50 and D65 are the two chromaticities everyone tabulates.
    #[test]
    fn the_locus_matches_published_chromaticities() {
        // D65: x = 0.31272, y = 0.32903.  D50: x = 0.34567, y = 0.35850.
        let (x, y) = locus_xy(6504.0);
        assert!((x - 0.31272).abs() < 0.002, "D65 x was {x}");
        assert!((y - 0.32903).abs() < 0.002, "D65 y was {y}");
        let (x, y) = locus_xy(5003.0);
        assert!((x - 0.34567).abs() < 0.002, "D50 x was {x}");
        assert!((y - 0.35850).abs() < 0.002, "D50 y was {y}");
    }

    /// ⚠ Warmer must mean *more red*. This is the direction that is easy to invert with nothing
    /// failing, because every other test here is symmetric in it.
    #[test]
    fn a_lower_temperature_is_redder() {
        let (warm, _) = locus_xy(2800.0);
        let (cool, _) = locus_xy(9000.0);
        assert!(
            warm > cool,
            "2800K ({warm}) should sit redder than 9000K ({cool})"
        );
    }

    /// The identity the whole feature rests on: asking for the temperature a source is already at
    /// must change nothing.
    #[test]
    fn the_baseline_temperature_gives_unit_gains() {
        let g = gains_for(BASELINE_KELVIN, 0.0, &crate::spaces::SRGB_TO_XYZ);
        for c in g {
            assert!((c - 1.0).abs() < 0.01, "gains were {g:?}");
        }
    }

    /// ⚠ Tint moves perpendicular to the locus, so at zero it must contribute nothing — or every
    /// Kelvin value would carry a hidden cast.
    #[test]
    fn zero_tint_leaves_the_locus_untouched() {
        let base = locus_xy(5500.0);
        let (x, y) = with_tint(base, 0.0);
        assert!((x - base.0).abs() < 1e-6 && (y - base.1).abs() < 1e-6);
    }

    /// ⚠ And a non-zero tint must actually move it, in opposite directions for opposite signs.
    #[test]
    fn tint_moves_the_chromaticity_both_ways() {
        let base = locus_xy(5500.0);
        let m = with_tint(base, 60.0);
        let g = with_tint(base, -60.0);
        assert!((m.1 - base.1).abs() > 1e-4, "magenta did not move");
        assert!(
            (m.1 - base.1).signum() != (g.1 - base.1).signum(),
            "green and magenta moved the same way"
        );
    }

    /// ⚠ Gains are luminance-normalised: a white-balance change moves colour, not brightness.
    /// Without this a warm setting also darkens the photograph.
    #[test]
    fn gains_preserve_luminance() {
        for k in [2800.0, 5500.0, 9000.0] {
            let g = gains_for(k, 0.0, &crate::spaces::SRGB_TO_XYZ);
            let lum = crate::luminance(g);
            assert!((lum - 1.0).abs() < 0.01, "at {k}K luminance was {lum}");
        }
    }

    /// ⚠ A singular matrix must degrade to the identity, not to `NaN` gains that would propagate
    /// through the whole image. A hand-edited profile can produce one.
    #[test]
    fn a_singular_matrix_gives_unit_gains() {
        let singular = [[1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]];
        assert_eq!(gains_for(5000.0, 0.0, &singular), [1.0, 1.0, 1.0]);
    }
}
