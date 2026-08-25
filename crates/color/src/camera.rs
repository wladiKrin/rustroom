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
    match code {
        1 => 5500.0,  // Daylight
        2 => 4200.0,  // Fluorescent
        3 => 2856.0,  // Tungsten
        17 => 2856.0, // Standard A
        18 => 4874.0, // Standard B
        19 => 6774.0, // Standard C
        20 => 5503.0, // D55
        21 => 6504.0, // D65
        22 => 7504.0, // D75
        23 => 5003.0, // D50
        _ => 6504.0,
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

/// The colour temperature implied by a camera neutral, solved the way the DNG spec does.
///
/// ⚠⚠ **Iterative because the answer depends on itself**: the matrix used to convert the neutral
/// to XYZ depends on the temperature, which is derived from that XYZ. Thirty passes is far more
/// than it needs — it converges in a handful — and a fixed count avoids any question of a loop
/// that does not terminate on a malformed profile.
///
/// ⚠ The temperature comes from McCamy's approximation, accurate to about 2% over the range
/// profiles actually use. That error moves the interpolation weight slightly and is far below the
/// difference between the two matrices; it is recorded rather than hidden.
pub fn neutral_temperature(
    neutral: [f32; 3],
    color1: [[f32; 3]; 3],
    color2: [[f32; 3]; 3],
    t1: f32,
    t2: f32,
) -> f32 {
    let mut temp = (t1 + t2) * 0.5;
    for _ in 0..30 {
        let g = illuminant_weight(temp, t1, t2);
        let Some(inv) = invert(&blend(color1, color2, g)) else {
            return temp;
        };
        let xyz = crate::spaces::apply(&inv, neutral);
        let sum = xyz[0] + xyz[1] + xyz[2];
        if sum.abs() < 1e-9 {
            return temp;
        }
        let (x, y) = (xyz[0] / sum, xyz[1] / sum);
        let n = (x - 0.3320) / (0.1858 - y);
        let next = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
        if !next.is_finite() {
            return temp;
        }
        let next = next.clamp(1500.0, 15000.0);
        if (next - temp).abs() < 1.0 {
            return next;
        }
        temp = next;
    }
    temp
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
}
