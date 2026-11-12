//! The camera → working-space transform.
//!
//! ⚠ This is the **input** side of colour management: it decides what the sensor's numbers mean
//! before any edit touches them. It lives in `color` so it can be tested without libraw, and is
//! applied by `raw-decode`, which is the only component that knows which camera took the picture.

use crate::spaces::SRGB_TO_PROPHOTO;

/// Scale each row so it sums to 1.
///
/// ⚠⚠ **This is what makes camera white land on output white**, and it is dcraw's own step —
/// omitting it gives every image a colour cast that is subtle enough to ship. A row that sums to
/// zero is left alone rather than divided by it: a NaN would compare false against every clamp
/// downstream and show as black.
///
/// ⚠ libraw's `rgb_cam` arrives already normalised, so this is a no-op there — measured, not
/// assumed. It earns its place with C2's DCP matrices, which are not.
pub fn normalize_rows(mut m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    for row in m.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-9 {
            for v in row.iter_mut() {
                *v /= sum;
            }
        }
    }
    m
}

/// Compose a camera → linear **sRGB** matrix into a camera → linear **ProPhoto** one.
///
/// ⚠⚠ The sRGB step is not an arbitrary waypoint — it is libraw's own convention, established by
/// probing rather than reasoning. `rgb_cam` is identical whatever `output_color` is set to, and
/// its rows sum to 1, because dcraw applies the output space *separately*
/// (`out_cam = out_rgb[output] × rgb_cam`). So `rgb_cam` is camera → linear sRGB, and this
/// supplies the second half from our own matrices instead of dcraw's.
pub fn camera_to_prophoto(rgb_cam: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let cam = normalize_rows(rgb_cam);
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| SRGB_TO_PROPHOTO[i][k] * cam[k][j]).sum();
        }
    }
    out
}

/// Correlated colour temperature, in kelvin, for an EXIF light-source code.
///
/// ⚠ Only the codes that appear in real profiles. An unknown code falls back to D65 rather than
/// failing: a slightly wrong illuminant renders a slightly wrong colour, where a hard error would
/// make a whole camera unusable.
pub fn illuminant_temperature(code: u16) -> f32 {
    // ⚠⚠ M10.13a: Adobe's DNG SDK's values (`dng_camera_profile::IlluminantToTemperature`), not the
    // illuminants' measured CCTs — the profile was calibrated by tools that blend on these, and 2856 against 2850 or
    // 6774 against 6500 (Standard C) moves the weight. A fluorescent class is the middle of its band.
    match code {
        3 | 17 => 2850.0,                  // Tungsten, Standard A
        24 => 3200.0,                      // ISO studio tungsten
        23 => 5000.0,                      // D50
        1 | 4 | 9 | 18 | 20 => 5500.0,     // Daylight, Flash, Fine weather, Standard B, D55
        10 | 19 | 21 => 6500.0,            // Cloudy, Standard C, D65
        11 | 22 => 7500.0,                 // Shade, D75
        12 => (5700.0 + 7100.0) * 0.5,     // Daylight fluorescent
        13 => (4600.0 + 5500.0) * 0.5,     // Day white fluorescent
        2 | 14 => (3800.0 + 4500.0) * 0.5, // Fluorescent, Cool white fluorescent
        15 => (3250.0 + 3800.0) * 0.5,     // White fluorescent
        16 => (2600.0 + 3250.0) * 0.5,     // Warm white fluorescent
        // ⚠ Unknown: the SDK treats the profile as single-illuminant; D65 keeps ours dual and is the commonest.
        _ => 6500.0,
    }
}

/// The weight for illuminant 1, given a temperature and the two calibration temperatures.
///
/// ⚠⚠ **Interpolated in reciprocal temperature (mireds), not linearly in kelvin** — that is the
/// DNG specification's rule, and linear-in-kelvin is the obvious wrong implementation. It agrees
/// at the endpoints and is visibly off in the middle, which is the worst way to be wrong.
///
/// Clamped rather than extrapolated: outside the calibrated range the nearer matrix is the best
/// available answer, and extrapolating two measured matrices produces nonsense.
pub fn illuminant_weight(temp: f32, temp1: f32, temp2: f32) -> f32 {
    if (temp1 - temp2).abs() < 1e-6 {
        return 1.0;
    }
    let (lo, hi) = (temp1.min(temp2), temp1.max(temp2));
    if temp <= lo {
        return if temp1 < temp2 { 1.0 } else { 0.0 };
    }
    if temp >= hi {
        return if temp1 < temp2 { 0.0 } else { 1.0 };
    }
    ((1.0 / temp - 1.0 / temp2) / (1.0 / temp1 - 1.0 / temp2)).clamp(0.0, 1.0)
}

/// D50 as the DNG SDK's PCS white: the XYZ of chromaticity (0.3457, 0.3585).
const PCS_WHITE: [f64; 3] = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];

/// A profile's `ColorMatrix` as Adobe's DNG SDK holds it (`dng_camera_profile::NormalizeColorMatrix`): scaled so D50
/// white maps to a camera value whose largest channel is 1 — unless it already is, within 1 % — then rounded to four
/// decimals.
///
/// ⚠⚠ M10.13a: the two calibrations are BLENDED, so scaling one and not the other moves every blend between them.
/// The X-T4's Standard A matrix maps D50 to 1.27 and is scaled by 1/1.27; read raw, a tungsten-ish neutral solved
/// 2.5 % hot of the SDK.
pub fn normalize_color_matrix(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let coord = m.map(|r| (0..3).map(|k| f64::from(r[k]) * PCS_WHITE[k]).sum::<f64>());
    let max = coord.iter().cloned().fold(f64::MIN, f64::max);
    let scale = if max > 0.0 && !(0.99..=1.01).contains(&max) {
        1.0 / max
    } else {
        1.0
    };
    m.map(|r| r.map(|v| ((f64::from(v) * scale * 10000.0).round() / 10000.0) as f32))
}

/// A profile's `ForwardMatrix` as the DNG SDK holds it (`NormalizeForwardMatrix`): each row scaled so camera white
/// `(1, 1, 1)` lands exactly on D50 — a profile's own rows sum to it only to its printed precision.
pub fn normalize_forward_matrix(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = m;
    for (i, row) in out.iter_mut().enumerate() {
        let sum: f64 = row.iter().map(|v| f64::from(*v)).sum();
        if sum.abs() > 1e-9 {
            let k = PCS_WHITE[i] / sum;
            *row = row.map(|v| (f64::from(v) * k) as f32);
        }
    }
    out
}

/// A convex combination of two matrices: `g` of `a`, `1 - g` of `b`.
pub fn blend(a: [[f32; 3]; 3], b: [[f32; 3]; 3], g: f32) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = g * a[i][j] + (1.0 - g) * b[i][j];
        }
    }
    out
}

/// Compose a DCP `ForwardMatrix` (camera → XYZ D50) into camera → linear ProPhoto.
pub fn forward_to_prophoto(forward: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3)
                .map(|k| crate::spaces::XYZ_D50_TO_PROPHOTO[i][k] * forward[k][j])
                .sum();
        }
    }
    out
}

/// 3×3 inverse, or `None` when the matrix is singular.
pub fn invert(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let mut out = [[0.0f32; 3]; 3];
    // ⚠ Indexed rather than iterated because the write is **transposed** (`out[j][i]`): that is
    // the adjugate, and iterating `out` in order would silently produce its transpose.
    #[allow(clippy::needless_range_loop)]
    for i in 0..3 {
        for j in 0..3 {
            let sub: Vec<f32> = (0..3)
                .filter(|r| *r != i)
                .flat_map(|r| (0..3).filter(move |c| *c != j).map(move |c| m[r][c]))
                .collect();
            let cof = sub[0] * sub[3] - sub[1] * sub[2];
            out[j][i] = cof * if (i + j) % 2 == 0 { 1.0 } else { -1.0 } / det;
        }
    }
    Some(out)
}

/// dcraw's own linear sRGB (D65) → XYZ, which `cam_xyz_coeff` multiplies by.
///
/// ⚠ Deliberately **not** [`crate::spaces::SRGB_TO_XYZ`]: these are dcraw's digits, a few 1e-4 off
/// ours, and [`rgb_cam_from_cam_xyz`] exists to reproduce the matrix libraw computes for a camera.
const DCRAW_XYZ_RGB: [[f64; 3]; 3] = [
    [0.412453, 0.357580, 0.180423],
    [0.212671, 0.715160, 0.072169],
    [0.019334, 0.119193, 0.950227],
];

/// Camera → linear sRGB from a DNG-style `ColorMatrix` (XYZ → camera) — dcraw's `cam_xyz_coeff`.
///
/// `cam_rgb = cam_xyz · xyz_rgb`, each row scaled to sum to 1 (so camera white lands on output
/// white), then inverted. It is the matrix libraw hands back as `rgb_cam` for a raw it reads, which
/// is what lets a DNG Rustroom wrote (M10.17a) render exactly as the raw it came from.
/// `None` when the matrix is singular.
pub fn rgb_cam_from_cam_xyz(cam_xyz: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let mut cam_rgb = [[0.0f32; 3]; 3];
    for (i, row) in cam_rgb.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3)
                .map(|k| cam_xyz[i][k] as f64 * DCRAW_XYZ_RGB[k][j])
                .sum::<f64>() as f32;
        }
    }
    if cam_rgb.iter().any(|r| r.iter().sum::<f32>().abs() < 1e-9) {
        return None;
    }
    invert(&normalize_rows(cam_rgb))
}

/// An XYZ → camera matrix that [`rgb_cam_from_cam_xyz`] turns back into `rgb_cam` — for a file
/// libraw gives a camera → sRGB matrix and no `cam_xyz` (a DNG, M10.17a): `rgb_cam⁻¹ · xyz_rgb⁻¹`.
/// Its rows are scaled so the camera's reading of sRGB white is 1 in each channel.
pub fn cam_xyz_from_rgb_cam(rgb_cam: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let cam_rgb = invert(rgb_cam)?;
    let xyz_rgb = DCRAW_XYZ_RGB.map(|r| r.map(|v| v as f32));
    let rgb_xyz = invert(&xyz_rgb)?;
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| cam_rgb[i][k] * rgb_xyz[k][j]).sum();
        }
    }
    Some(out)
}

/// The colour temperature implied by a camera neutral, solved the way the DNG spec does.
///
/// ⚠⚠ **Iterative because the answer depends on itself**: the matrix used to convert the neutral
/// to XYZ depends on the temperature, which is derived from that XYZ. Thirty passes is far more
/// than it needs — it converges in a handful — and a fixed count avoids any question of a loop
/// that does not terminate on a malformed profile.
///
/// ⚠⚠ The temperature is Robertson's (`crate::robertson`), as in Adobe's DNG SDK — since M10.13a, measured against
/// it. McCamy's approximation, used before, was called "about 2 %" here and read a green-tinted neutral 23 % off.
pub fn neutral_temperature(
    neutral: [f32; 3],
    color1: [[f32; 3]; 3],
    color2: [[f32; 3]; 3],
    t1: f32,
    t2: f32,
) -> f32 {
    // ⚠⚠ M10.13a: the DNG SDK's own iteration (`dng_color_spec::NeutralToXY`) — from D50's chromaticity, the white
    // the blended matrix maps the neutral back to, until it moves less than 1e-7, averaging the last two if it never
    // settles — with the temperature by Robertson's method, as the SDK computes it.
    // ⚠ Normalised as the SDK holds them — idempotent, so a caller that already did it loses nothing.
    let (color1, color2) = (
        normalize_color_matrix(color1),
        normalize_color_matrix(color2),
    );
    let (mut x, mut y) = (0.3457f64, 0.3585f64);
    for pass in 0..30 {
        let temp = crate::robertson::temperature(x, y) as f32;
        let g = illuminant_weight(temp, t1, t2);
        let Some(inv) = invert(&blend(color1, color2, g)) else {
            return temp;
        };
        let xyz = crate::spaces::apply(&inv, neutral).map(f64::from);
        let sum = xyz[0] + xyz[1] + xyz[2];
        if sum.abs() < 1e-12 || !sum.is_finite() {
            return temp;
        }
        let (nx, ny) = (xyz[0] / sum, xyz[1] / sum);
        if (nx - x).abs() + (ny - y).abs() < 1e-7 {
            return crate::robertson::temperature(nx, ny) as f32;
        }
        if pass == 29 {
            return crate::robertson::temperature((x + nx) * 0.5, (y + ny) * 0.5) as f32;
        }
        (x, y) = (nx, ny);
    }
    unreachable!("the loop returns on its last pass")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::apply;

    /// The X-T4 profile's real values, from the spec.
    const FM1: [[f32; 3]; 3] = [
        [0.6666, 0.1486, 0.1491],
        [0.2533, 0.7037, 0.0430],
        [0.0015, 0.0350, 0.7886],
    ];
    const FM2: [[f32; 3]; 3] = [
        [0.6140, 0.1279, 0.2224],
        [0.2698, 0.6609, 0.0693],
        [0.0002, 0.0160, 0.8088],
    ];

    /// The X-T4 profile's ColorMatrices — the same file as [`FM1`]/[`FM2`].
    const CM1: [[f32; 3]; 3] = [
        [1.8038, -0.902, 0.0103],
        [-0.3719, 1.4063, 0.2712],
        [-0.0196, 0.0991, 0.6531],
    ];
    const CM2: [[f32; 3]; 3] = [
        [0.9798, -0.3701, -0.0878],
        [-0.446, 1.2149, 0.2613],
        [-0.0705, 0.1228, 0.6461],
    ];

    /// ⚠⚠ M10.13a's oracle, CAPTURED: Adobe's DNG SDK 1.7.1 (`dng_color_spec::NeutralToXY`, `dng_temperature`,
    /// `SetWhiteXY`) on RawTherapee's `FUJIFILM X-T4.dcp` (byte-identical to the author's), 2026-11-12 — a C++ harness
    /// linked against the SDK, run by the author; `raw-decode`'s `dcp_oracle` example compares all 49 of its white
    /// balances. Per row: the camera neutral, the SDK's temperature, and its forward matrix on white-balanced camera
    /// RGB with Y's row scaled to 1. ⚠ The first row was 23 % off under McCamy (3369 K against 4403 K).
    // ⚠ The digits as the harness printed them, kept so the fixture can be checked against its source.
    #[allow(clippy::excessive_precision)]
    const SDK: [([f32; 3], f32, [f32; 9]); 5] = [
        (
            [0.833333, 1.0, 0.833333],
            4403.015,
            // ⚠ Temperature only, here and on two more green neutrals of the 49: the SDK's matrix there is no blend
            // of the two (its first row sums to 0.983, not D50's 0.964) — its camera white drifted from the neutral.
            [0.0; 9],
        ),
        (
            [0.833333, 1.0, 0.555556],
            3597.748,
            [
                0.641169258,
                0.138594411,
                0.184529668,
                0.261273539,
                0.683014099,
                0.055712362,
                0.000871664546,
                0.0258175996,
                0.798421497,
            ],
        ),
        (
            [0.666667, 1.0, 0.666667],
            4414.436,
            [
                0.623236498,
                0.131539809,
                0.209513607,
                0.266894599,
                0.668428857,
                0.0646765435,
                0.000428601249,
                0.0193426615,
                0.805345925,
            ],
        ),
        ([0.555556, 1.0, 0.833333], 6122.389, [0.0; 9]),
        ([0.357143, 1.0, 0.833333], 9761.624, [0.0; 9]),
    ];

    #[test]
    fn the_temperature_and_matrix_match_adobes_dng_sdk() {
        let (t1, t2) = (illuminant_temperature(17), illuminant_temperature(23));
        for (neutral, temp, fm) in SDK {
            let ours = neutral_temperature(neutral, CM1, CM2, t1, t2);
            assert!(
                (ours - temp).abs() / temp < 0.002,
                "{neutral:?}: ours {ours} K, the SDK {temp} K"
            );
            if fm[0] != 0.0 {
                let m = blend(
                    normalize_forward_matrix(FM1),
                    normalize_forward_matrix(FM2),
                    illuminant_weight(ours, t1, t2),
                );
                let s: f32 = m[1].iter().sum();
                for k in 0..9 {
                    assert!(
                        (m[k / 3][k % 3] / s - fm[k]).abs() < 1e-4,
                        "{neutral:?} [{k}]: {} against {}",
                        m[k / 3][k % 3] / s,
                        fm[k]
                    );
                }
            }
        }
    }

    #[test]
    fn the_blend_reaches_both_endpoints_exactly() {
        // ⚠ An interpolation wrong in the middle is often still right at the ends — but one wrong
        // at the ends is wrong everywhere. These pin the cheap half exactly; the middle is
        // validated against RawTherapee, which is a different kind of check.
        assert_eq!(blend(FM1, FM2, 1.0), FM1);
        assert_eq!(blend(FM1, FM2, 0.0), FM2);
    }

    #[test]
    fn a_blended_forward_matrix_still_lands_on_d50_white() {
        // ⚠ The DNG constraint must survive interpolation, not merely hold at the endpoints — it
        // does, because the constraint is linear in the matrix. A failure means `blend` is not a
        // convex combination.
        for g in [0.0, 0.3, 0.5, 0.8, 1.0] {
            let m = blend(FM1, FM2, g);
            let want = [0.9642, 1.0, 0.8249];
            for (i, row) in m.iter().enumerate() {
                let sum: f32 = row.iter().sum();
                assert!((sum - want[i]).abs() < 2e-3, "g={g} row {i} sums to {sum}");
            }
        }
    }

    #[test]
    fn the_weight_follows_the_dng_reciprocal_rule() {
        // ⚠⚠ DNG interpolates in **reciprocal** temperature (mireds), not linearly in kelvin.
        // Linear-in-kelvin agrees at both endpoints and is visibly wrong in between — the worst
        // way to be wrong, and the reason this test probes the middle rather than the ends.
        // Between 2856 K and 5000 K the mired midpoint is ~3635 K, not 3928 K.
        assert!((illuminant_weight(2856.0, 2856.0, 5000.0) - 1.0).abs() < 1e-6);
        assert!((illuminant_weight(5000.0, 2856.0, 5000.0) - 0.0).abs() < 1e-6);
        let mid = illuminant_weight(3635.0, 2856.0, 5000.0);
        assert!((mid - 0.5).abs() < 0.02, "mired midpoint gave {mid}");
        // Clamped outside the calibrated range rather than extrapolated.
        assert_eq!(illuminant_weight(2000.0, 2856.0, 5000.0), 1.0);
        assert_eq!(illuminant_weight(9000.0, 2856.0, 5000.0), 0.0);
    }

    #[test]
    fn the_illuminant_choice_is_invisible_on_neutrals_and_visible_on_colour() {
        // ⚠⚠ **Why a neutral test can never validate the interpolation.** Both ForwardMatrices
        // have rows summing to D50 white by construction, so they agree on a grey to 1e-4 — an
        // inverted illuminant choice is completely invisible there. On coloured content they
        // differ by 0.02–0.03, which is what makes "our render is within 0.002 of libraw's"
        // evidence that the right matrix was selected rather than a coincidence.
        //
        // This test exists to stop anyone concluding from the neutral test alone that the
        // interpolation is verified. It is not; see the spec's note on the missing oracle.
        let (m1, m2) = (forward_to_prophoto(FM1), forward_to_prophoto(FM2));
        let grey = (0..3)
            .map(|i| (apply(&m1, [0.5, 0.5, 0.5])[i] - apply(&m2, [0.5, 0.5, 0.5])[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            grey < 1e-3,
            "the two matrices should agree on a neutral, got {grey}"
        );

        for c in [[0.6, 0.45, 0.35], [0.3, 0.45, 0.7]] {
            let d = (0..3)
                .map(|i| (apply(&m1, c)[i] - apply(&m2, c)[i]).abs())
                .fold(0.0f32, f32::max);
            assert!(
                d > 0.01,
                "the illuminant choice should matter on colour, {c:?} gave {d}"
            );
        }
    }

    #[test]
    fn a_neutral_camera_pixel_is_neutral_through_a_forward_matrix() {
        // ⚠ The point of a ForwardMatrix: its rows sum to D50 white, so a neutral camera reading
        // lands on D50 white in XYZ and therefore neutral in ProPhoto, which is a D50 space.
        for m in [FM1, FM2] {
            let out = apply(&forward_to_prophoto(m), [1.0, 1.0, 1.0]);
            assert!(
                (out[0] - out[1]).abs() < 2e-3 && (out[1] - out[2]).abs() < 2e-3,
                "neutral in, {out:?} out"
            );
        }
    }

    #[test]
    fn invert_round_trips_and_refuses_a_singular_matrix() {
        let m = [
            [0.6666, 0.1486, 0.1491],
            [0.2533, 0.7037, 0.043],
            [0.0015, 0.035, 0.7886],
        ];
        let inv = invert(&m).expect("invertible");
        for c in [[0.3, 0.6, 0.1], [0.9, 0.2, 0.5]] {
            let back = apply(&m, apply(&inv, c));
            for i in 0..3 {
                assert!(
                    (back[i] - c[i]).abs() < 1e-4,
                    "{c:?} round-tripped to {back:?}"
                );
            }
        }
        assert!(invert(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 1.0, 1.0]]).is_none());
    }

    #[test]
    fn the_neutral_solve_lands_between_the_calibration_temperatures() {
        // ⚠ A real X-T4 neutral (1/cam_mul from libraw's probe) must resolve to a temperature
        // inside the calibrated range — the iteration diverging or pinning to a clamp is exactly
        // the failure mode, and either would show here.
        const CM1: [[f32; 3]; 3] = [
            [1.8038, -0.9020, 0.0103],
            [-0.3719, 1.4063, 0.2712],
            [-0.0196, 0.0991, 0.6531],
        ];
        const CM2: [[f32; 3]; 3] = [
            [0.9798, -0.3701, -0.0878],
            [-0.4460, 1.2149, 0.2613],
            [-0.0705, 0.1228, 0.6461],
        ];
        let cam_mul = [597.0f32, 302.0, 482.0];
        let neutral = [1.0 / cam_mul[0], 1.0 / cam_mul[1], 1.0 / cam_mul[2]];
        let t = neutral_temperature(neutral, CM1, CM2, 2856.0, 5003.0);
        assert!(t > 1500.0 && t < 15000.0, "solve produced {t}");
        eprintln!("X-T4 neutral solved to {t:.0} K");
    }

    /// libraw's real `rgb_cam` for a Fujifilm X-T4, from `examples/color_probe.rs`.
    const X_T4: [[f32; 3]; 3] = [
        [1.2582287, 0.052999634, -0.3112283],
        [-0.11689235, 1.5896847, -0.4727924],
        [-0.09838579, -0.50626373, 1.6046495],
    ];

    #[test]
    fn every_row_sums_to_one() {
        // ⚠ The property that makes camera white land on output white. Without it every image
        // carries a colour cast — subtle enough to ship, bad enough to matter.
        let m = normalize_rows([[0.8, 0.1, 0.2], [0.05, 0.9, 0.1], [0.0, 0.1, 0.7]]);
        for (i, row) in m.iter().enumerate() {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "row {i} sums to {sum}");
        }
    }

    #[test]
    fn a_neutral_camera_pixel_stays_neutral() {
        // ⚠ The same property expressed the way it is actually observed: equal camera channels
        // must produce equal output channels. A row-sum test alone would pass a matrix whose
        // rows were individually normalised but wrongly ordered.
        let m = normalize_rows([[0.8, 0.1, 0.2], [0.05, 0.9, 0.1], [0.0, 0.1, 0.7]]);
        let out = apply(&m, [0.5, 0.5, 0.5]);
        assert!(
            (out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6,
            "neutral in, {out:?} out"
        );
    }

    #[test]
    fn normalizing_leaves_an_already_normal_matrix_alone() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(normalize_rows(id), id);
    }

    #[test]
    fn librarys_own_matrix_is_already_normalised() {
        // ⚠ Measured, not assumed — this is why `normalize_rows` is a no-op on the libraw path
        // and why it must still exist for C2, whose DCP matrices are not normalised.
        for row in X_T4 {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "row sums to {sum}");
        }
    }

    #[test]
    fn a_degenerate_row_is_left_alone_rather_than_producing_nan() {
        // ⚠ A zero row would divide by zero and poison the whole image with NaN, which compares
        // false against every clamp and shows as black. Failing toward "unchanged" is the safe
        // direction, the same call `PrefixKey` makes for NaN.
        let m = normalize_rows([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(m[0].iter().all(|v| v.is_finite()), "got {m:?}");
    }

    #[test]
    fn a_neutral_camera_pixel_is_neutral_in_prophoto_too() {
        // ⚠ The composition must preserve white as well as each half does. sRGB white maps to
        // ProPhoto white, so a neutral sensor reading has to come out neutral — this is what a
        // transposed or mis-ordered composition would break.
        let out = apply(&camera_to_prophoto(X_T4), [0.5, 0.5, 0.5]);
        assert!(
            (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
            "neutral in, {out:?} out"
        );
    }

    #[test]
    fn the_composition_agrees_with_applying_the_halves_in_turn() {
        // ⚠ The load-bearing test: composing two matrices is where index order goes wrong, and a
        // transposed composition still preserves neutrals — so `a_neutral_camera_pixel...` above
        // cannot detect it. Applying the halves sequentially is an independent path.
        for c in [[0.4, 0.2, 0.1], [0.05, 0.6, 0.9], [0.9, 0.9, 0.2]] {
            let stepwise = apply(&SRGB_TO_PROPHOTO, apply(&normalize_rows(X_T4), c));
            let composed = apply(&camera_to_prophoto(X_T4), c);
            for i in 0..3 {
                assert!(
                    (stepwise[i] - composed[i]).abs() < 1e-5,
                    "channel {i}: stepwise {} vs composed {}",
                    stepwise[i],
                    composed[i]
                );
            }
        }
    }

    /// M10.17a: a camera whose native space IS sRGB has `rgb_cam` = the identity — dcraw's own
    /// `cam_xyz_coeff` reduces to it. ⚠ Within 2e-3, not exactly: dcraw's `xyz_rgb` constants are
    /// its own (0.412453…), a few 1e-4 off `SRGB_TO_XYZ`, and the function uses dcraw's on purpose
    /// so it reproduces libraw's matrix for a real camera.
    #[test]
    fn rgb_cam_from_an_srgb_camera_is_the_identity() {
        let cam_xyz = invert(&crate::spaces::SRGB_TO_XYZ).unwrap();
        let m = rgb_cam_from_cam_xyz(&cam_xyz).expect("invertible");
        for (i, row) in m.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 2e-3, "{m:?}");
            }
        }
    }

    /// Camera white lands on output white: every row of `rgb_cam` sums to 1, for any matrix.
    #[test]
    fn rows_of_rgb_cam_sum_to_one() {
        let cam_xyz = [
            [1.3426, -0.6334, -0.1177],
            [-0.4244, 1.2136, 0.2371],
            [0.058, 0.1303, 0.598],
        ];
        let m = rgb_cam_from_cam_xyz(&cam_xyz).expect("invertible");
        for row in m {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{m:?}");
        }
    }

    /// M10.17a: for a DNG, libraw computes `rgb_cam` and leaves `cam_xyz` empty; deriving one back
    /// must give the same `rgb_cam` through dcraw's step.
    #[test]
    fn cam_xyz_from_rgb_cam_round_trips() {
        let cam_xyz = [
            [1.3426, -0.6334, -0.1177],
            [-0.4244, 1.2136, 0.2371],
            [0.058, 0.1303, 0.598],
        ];
        let rgb_cam = rgb_cam_from_cam_xyz(&cam_xyz).unwrap();
        let back = rgb_cam_from_cam_xyz(&cam_xyz_from_rgb_cam(&rgb_cam).unwrap()).unwrap();
        for (a, b) in back.iter().flatten().zip(rgb_cam.iter().flatten()) {
            assert!((a - b).abs() < 1e-4, "{back:?} vs {rgb_cam:?}");
        }
    }
}
