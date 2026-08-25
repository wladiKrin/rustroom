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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::apply;

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
