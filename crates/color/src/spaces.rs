//! Linear RGB working spaces and the transforms between them.
//!
//! ⚠ Everything here is **linear light**. The sRGB *transfer* functions live in `lib.rs`; these
//! matrices only change primaries, never encoding. Applying one to gamma-encoded values is
//! meaningless and will not fail loudly.
//!
//! ⚠ The matrices are composed offline from primaries, white points and a Bradford adaptation,
//! then checked in the tests against an independent path. They are `const` rather than computed
//! at startup because they never change and a per-pixel path should not pay for a lazy init.
#![allow(clippy::excessive_precision)]

/// Linear ProPhoto (ROMM, D50) → linear sRGB (Rec.709, D65), Bradford-adapted.
///
/// ⚠ Produces **negative** components for colours outside sRGB's gamut. That is correct and must
/// not be clamped here: the output transform decides what to do about gamut, and clamping in the
/// matrix would silently discard the very data this working space exists to keep.
pub const PROPHOTO_TO_SRGB: [[f32; 3]; 3] = [
    [2.0342891610, -0.7275078194, -0.3067813416],
    [-0.2288019903, 1.2317255449, -0.0029235546],
    [-0.0085622791, -0.1532652876, 1.1618275667],
];

/// Linear sRGB → linear ProPhoto. The inverse of [`PROPHOTO_TO_SRGB`].
pub const SRGB_TO_PROPHOTO: [[f32; 3]; 3] = [
    [0.5292929985, 0.3301161484, 0.1405908531],
    [0.0983600854, 0.8734699203, 0.0281699943],
    [0.0168761198, 0.1176587379, 0.8654651424],
];

/// Apply a 3×3 matrix to a pixel.
#[inline]
pub fn apply(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

/// Linear ProPhoto → linear sRGB.
#[inline]
pub fn prophoto_to_srgb(c: [f32; 3]) -> [f32; 3] {
    apply(&PROPHOTO_TO_SRGB, c)
}

/// Linear sRGB → linear ProPhoto.
#[inline]
pub fn srgb_to_prophoto(c: [f32; 3]) -> [f32; 3] {
    apply(&SRGB_TO_PROPHOTO, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < tol)
    }

    #[test]
    fn the_two_matrices_are_inverses() {
        // ⚠ Round-tripping several colours, not one: a matrix pair can be inverse on the
        // diagonal and wrong off it, and a grey probe would never notice — grey is the one
        // colour every sane matrix preserves.
        for c in [
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.9, 0.3, 0.1],
            [0.02, 0.7, 0.45],
        ] {
            let back = prophoto_to_srgb(srgb_to_prophoto(c));
            assert!(close(back, c, 1e-4), "{c:?} round-tripped to {back:?}");
        }
    }

    #[test]
    fn white_maps_to_white() {
        // ⚠ The check that the Bradford adaptation is present and the right way round. Without
        // it, D50 white lands off-white in sRGB — a warm cast on *every* image, which is subtle
        // enough to ship and awful enough to matter.
        let w = prophoto_to_srgb([1.0, 1.0, 1.0]);
        assert!(
            close(w, [1.0, 1.0, 1.0], 1e-3),
            "ProPhoto white became {w:?}"
        );
        let w = srgb_to_prophoto([1.0, 1.0, 1.0]);
        assert!(close(w, [1.0, 1.0, 1.0], 1e-3), "sRGB white became {w:?}");
    }

    #[test]
    fn prophoto_holds_colours_srgb_cannot() {
        // ⚠ The point of the whole working space, as a test. A saturated sRGB green is *inside*
        // ProPhoto, so it converts to components all within [0, 1]; the reverse — ProPhoto's own
        // primary green — falls outside sRGB and must come back with a negative component.
        let inside = srgb_to_prophoto([0.0, 1.0, 0.0]);
        assert!(
            inside.iter().all(|v| (-0.001..=1.001).contains(v)),
            "sRGB green should fit inside ProPhoto, got {inside:?}"
        );
        let outside = prophoto_to_srgb([0.0, 1.0, 0.0]);
        assert!(
            outside.iter().any(|v| *v < -0.01),
            "ProPhoto green should be outside sRGB, got {outside:?}"
        );
    }

    #[test]
    fn the_matrix_does_not_clamp() {
        // ⚠ A guard, not a discovery. Someone "tidying" the conversion by clamping to [0, 1]
        // would destroy exactly the out-of-gamut data this space exists to carry, and every
        // other test here would still pass.
        let v = prophoto_to_srgb([0.0, 1.0, 0.0]);
        assert!(v.iter().any(|x| *x < 0.0), "negatives were clamped away");
        let v = prophoto_to_srgb([2.0, 2.0, 2.0]);
        assert!(v[0] > 1.5, "values above 1.0 were clamped away: {v:?}");
    }
}
