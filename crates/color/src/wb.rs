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

/// An illuminant, as the white-balance controls describe one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Illuminant {
    pub kelvin: f32,
    pub tint: f32,
}

impl Illuminant {
    pub const fn new(kelvin: f32, tint: f32) -> Self {
        Self { kelvin, tint }
    }
}

impl Default for Illuminant {
    /// D65 — a file the camera has already balanced, and the working buffer's own neutral.
    fn default() -> Self {
        Self::new(BASELINE_KELVIN, 0.0)
    }
}

/// The gains that re-balance a buffer from `baseline` to `declared`, in the space `rgb_to_xyz`
/// describes.
///
/// ⚠⚠ **THE STAGE OPERATES ON THE WORKING SPACE, NOT ON CAMERA RGB — which is why this is a ratio
/// of two illuminants and not [`gains_for`] of one.** `raw-decode` converts camera → linear
/// ProPhoto before the pipeline ever runs, so by the time `WhiteBalanceStage` multiplies anything,
/// a neutral object already reads as equal RGB. There is no camera space left to balance in. What
/// remains is an *adaptation*: the decode balanced the picture to `baseline`, and the slider says
/// the light was `declared` instead.
///
/// ⚠ It is exactly `[1, 1, 1]` when the two agree — `x / x` is 1 for any finite non-zero `x` — so
/// declaring a photo's own baseline is a no-op, which is what makes the slider's zero point mean
/// something. (The luminance normalisation can perturb that by ~1e-7; `None` in `Edits` inserts no
/// stage at all, so the exactly-neutral case never goes through here.)
///
/// ⚠ Direction, pinned because it reads backwards: a **lower** declared Kelvin is warmer light, so
/// more of it is divided out and the picture gets **bluer** — matching every other raw converter.
pub fn adaptation_gains(
    declared: Illuminant,
    baseline: Illuminant,
    rgb_to_xyz: &[[f32; 3]; 3],
) -> [f32; 3] {
    let Some(inv) = crate::camera::invert(rgb_to_xyz) else {
        return [1.0, 1.0, 1.0];
    };
    let rgb_of = |i: Illuminant| {
        let (x, y) = with_tint(locus_xy(i.kelvin), i.tint);
        if y.abs() < 1e-6 {
            return [1.0, 1.0, 1.0];
        }
        crate::spaces::apply(&inv, [x / y, 1.0, (1.0 - x - y) / y])
    };
    let (b, d) = (rgb_of(baseline), rgb_of(declared));
    // ⚠ A non-positive channel means the illuminant falls outside this space's gamut. Dividing by
    // it would give a **negative gain** and invert that channel across the whole photograph — a
    // failure that looks like a bug in something else entirely.
    let mut gains = [1.0f32; 3];
    for i in 0..3 {
        if b[i] > 1e-6 && d[i] > 1e-6 {
            gains[i] = b[i] / d[i];
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

/// The illuminant nearest a chromaticity: its Kelvin, and how far off the locus it sits.
///
/// ⚠ The inverse of [`locus_xy`] + [`with_tint`], and the reason both the eyedropper and a RAW's
/// as-shot baseline can share one implementation — they are the same question asked of a picked
/// pixel and of the camera's own neutral.
///
/// ⚠ Solved by scanning the locus in **mireds**, not Kelvin: equal steps in mireds are equal
/// perceptual steps, so a linear scan is uniform where a Kelvin scan would be dense at the
/// tungsten end and coarse at the shade end. Refined by bisection around the best coarse hit.
pub fn solve_xy((x, y): (f32, f32)) -> Illuminant {
    const MIN_MIRED: f32 = 1.0e6 / 25000.0;
    const MAX_MIRED: f32 = 1.0e6 / 1667.0;
    let d2 = |k: f32| {
        let (lx, ly) = locus_xy(k);
        (x - lx) * (x - lx) + (y - ly) * (y - ly)
    };
    let mut best = (f32::MAX, MIN_MIRED);
    let steps = 240;
    for i in 0..=steps {
        let m = MIN_MIRED + (MAX_MIRED - MIN_MIRED) * i as f32 / steps as f32;
        let d = d2(1.0e6 / m);
        if d < best.0 {
            best = (d, m);
        }
    }
    let step = (MAX_MIRED - MIN_MIRED) / steps as f32;
    let (mut lo, mut hi) = (best.1 - step, best.1 + step);
    for _ in 0..40 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if d2(1.0e6 / a) < d2(1.0e6 / b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let kelvin = (1.0e6 / ((lo + hi) * 0.5)).clamp(1667.0, 25000.0);

    // The signed offset along the perpendicular, in the units `with_tint` uses.
    let (lx, ly) = locus_xy(kelvin);
    let dy_dx = -6.000 * lx + 2.870;
    let n = (1.0 + dy_dx * dy_dx).sqrt();
    let (px, py) = (-dy_dx / n, 1.0 / n);
    let tint = ((x - lx) * px + (y - ly) * py) / TINT_SCALE;
    Illuminant::new(kelvin, tint)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PP: [[f32; 3]; 3] = crate::spaces::PROPHOTO_TO_XYZ;

    /// ⚠⚠ The property the whole slider rests on: declaring a photo's own baseline changes
    /// nothing. Without it the zero point of the control is arbitrary and every photo shifts the
    /// moment a Kelvin is set at all.
    #[test]
    fn declaring_the_baseline_is_a_no_op() {
        for k in [2800.0, 5000.0, 6504.0, 9000.0] {
            for t in [-40.0, 0.0, 25.0] {
                let i = Illuminant::new(k, t);
                let g = adaptation_gains(i, i, &PP);
                for c in g {
                    assert!((c - 1.0).abs() < 1e-5, "{k}K tint {t} gave {g:?}");
                }
            }
        }
    }

    /// ⚠ Direction, and it reads backwards until you say it out loud: a **lower** declared Kelvin
    /// means warmer light, so more warmth is divided out and the picture goes **bluer**. Every
    /// other raw converter behaves this way, and getting it inverted would still look like a
    /// working control.
    #[test]
    fn a_lower_declared_kelvin_cools_the_picture() {
        let base = Illuminant::default();
        let warm = adaptation_gains(Illuminant::new(3200.0, 0.0), base, &PP);
        let cool = adaptation_gains(Illuminant::new(9000.0, 0.0), base, &PP);
        assert!(
            warm[2] > warm[0],
            "3200K should raise blue over red: {warm:?}"
        );
        assert!(
            cool[0] > cool[2],
            "9000K should raise red over blue: {cool:?}"
        );
    }

    /// ⚠ Tint is the other axis and must not leak into the temperature one: positive is magenta,
    /// which means *less* green.
    #[test]
    fn positive_tint_is_magenta() {
        let base = Illuminant::default();
        let g = adaptation_gains(Illuminant::new(BASELINE_KELVIN, 60.0), base, &PP);
        assert!(g[1] < 1.0, "magenta tint should cut green: {g:?}");
    }

    /// ⚠⚠ The round trip that makes both the eyedropper and a RAW's as-shot baseline meaningful.
    /// It is only evidence *because* `the_locus_matches_published_chromaticities` pins the locus
    /// against published values — a self-consistent pair of wrong transforms round-trips happily.
    #[test]
    fn a_chromaticity_solves_back_to_the_illuminant_that_made_it() {
        for k in [2000.0, 3200.0, 5500.0, 6504.0, 12000.0] {
            for t in [-50.0, 0.0, 30.0] {
                let got = solve_xy(with_tint(locus_xy(k), t));
                assert!(
                    (got.kelvin - k).abs() < k * 0.02,
                    "{k}K tint {t} solved to {}K",
                    got.kelvin
                );
                assert!(
                    (got.tint - t).abs() < 6.0,
                    "{k}K tint {t} solved to tint {}",
                    got.tint
                );
            }
        }
    }

    /// ⚠ A neutral working-space pixel is D65 by construction of [`PROPHOTO_TO_XYZ`], so an
    /// eyedropper on an already-balanced photo must report the baseline and change nothing.
    #[test]
    fn a_neutral_working_space_pixel_solves_to_the_baseline() {
        let xyz = crate::spaces::apply(&PP, [0.5, 0.5, 0.5]);
        let s: f32 = xyz.iter().sum();
        let got = solve_xy((xyz[0] / s, xyz[1] / s));
        assert!(
            (got.kelvin - BASELINE_KELVIN).abs() < 300.0,
            "got {}K",
            got.kelvin
        );
        assert!(got.tint.abs() < 5.0, "got tint {}", got.tint);
    }

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
