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

/// Linear ProPhoto (ROMM, D50) → linear Display P3 (D65), Bradford-adapted.
///
/// ⚠ Display P3 differs from sRGB **only in its primaries**. Its transfer curve is the sRGB
/// one — see `OutputSpace::encode`.
pub const PROPHOTO_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [1.6325756087, -0.3797716185, -0.2528039902],
    [-0.1537004023, 1.1667025472, -0.0130021449],
    [0.0103931953, -0.0628073126, 1.0524141174],
];

/// Linear ProPhoto (ROMM, D50) → linear Adobe RGB (1998) (D65), Bradford-adapted.
pub const PROPHOTO_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [1.3896512482, -0.1694590769, -0.2201921712],
    [-0.2288257316, 1.2317425412, -0.0029168096],
    [-0.0176254437, -0.0962570231, 1.1138824667],
];

/// Linear sRGB → linear Display P3. Both are D65, so no chromatic adaptation is involved.
///
/// ⚠ Every entry is non-negative and no in-range input can produce a negative output, because
/// sRGB's gamut sits strictly *inside* P3 — the opposite of [`PROPHOTO_TO_SRGB`], which must
/// produce negatives. Do not copy a clamping decision between the two.
pub const SRGB_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [0.8224619687, 0.1775380313, 0.0000000000],
    [0.0331941989, 0.9668058011, 0.0000000000],
    [0.0170826307, 0.0723974407, 0.9105199286],
];

/// Linear sRGB → linear Adobe RGB (1998). Both are D65.
///
/// ⚠⚠ **The middle row really is `[0, 1, 0]`.** It looks like a copy-paste error and is not:
/// Adobe RGB shares sRGB's red and blue primaries and differs only in green, and the green
/// *channel value* comes through unchanged. Cross-checked independently — sRGB green expressed
/// here is `(0.2849, 1.0, 0.0412)`, whose luminance through Adobe's own Y coefficients
/// (0.2974 / 0.6273 / 0.0753) is 0.7151, matching sRGB green's known 0.7152.
pub const SRGB_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [0.7151256069, 0.2848743931, 0.0000000000],
    [0.0000000000, 1.0000000000, 0.0000000000],
    [0.0000000000, 0.0411619485, 0.9588380515],
];

/// Linear sRGB → XYZ (D65).
///
/// ⚠ Derived from the sRGB primaries and the D65 white point rather than transcribed, then checked
/// against the standard values — the same treatment the ProPhoto pair got, which is what showed a
/// 1e-4 difference there to be rounding rather than error.
///
/// ⚠ Its middle row is **Rec.709's** luminance weights, which is why it is not interchangeable with
/// `crate::luminance` — that one is ProPhoto's Y, for the working space.
pub const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.4123908, 0.3575843, 0.1804808],
    [0.2126390, 0.7151687, 0.0721923],
    [0.0193308, 0.1191948, 0.9505322],
];

/// Linear ProPhoto (the working space) → XYZ, **D65-referred**.
///
/// ⚠⚠ **D65, not ProPhoto's native D50, and that is the whole point of having it.** Composed as
/// `SRGB_TO_XYZ · PROPHOTO_TO_SRGB`, so it lands in the same XYZ frame as [`SRGB_TO_XYZ`] — which
/// is what makes "equal RGB in the working buffer" mean D65, the white a viewer actually sees
/// after the output transform's Bradford step. Verified: `[1, 1, 1]` maps to `(0.9505, 1.0,
/// 1.0891)`, i.e. xy `(0.31270, 0.32900)` against published D65 `(0.31272, 0.32903)`.
///
/// ⚠ Do **not** substitute the inverse of [`XYZ_D50_TO_PROPHOTO`]: that is the same space in a D50
/// frame, where the buffer's neutral reads as 5003 K instead of 6504 K. Both are correct
/// statements about ProPhoto; only this one answers "what illuminant does a neutral pixel show as
/// on screen", which is what a Kelvin slider is about.
pub const PROPHOTO_TO_XYZ: [[f32; 3]; 3] = [
    [0.7555608, 0.1127667, 0.0821283],
    [0.2683191, 0.7151304, 0.0165505],
    [0.0039137, -0.0129316, 1.0980757],
];

/// XYZ (D50) → linear ProPhoto (ROMM).
///
/// ⚠ **No chromatic adaptation, and that is not an omission.** ProPhoto *is* a D50 space, so a
/// D50 XYZ needs none — verified: D50 white `(0.9642, 1.0, 0.8249)` maps to exactly `(1, 1, 1)`.
/// Inserting a Bradford step here would be a second, wrong adaptation.
pub const XYZ_D50_TO_PROPHOTO: [[f32; 3]; 3] = [
    [1.3459461131, -0.2556023263, -0.0511079113],
    [-0.5446046140, 1.5081754886, 0.0205264640],
    [0.0000000000, 0.0000000000, 1.2122681537],
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

    /// ⚠⚠ The one property [`PROPHOTO_TO_XYZ`] exists for: a neutral pixel in the working buffer
    /// is **D65**, because the output transform's Bradford step is what a viewer's white actually
    /// is. Everything the Kelvin slider means rests on this — get the frame wrong and the
    /// slider's no-op point moves to 5003 K without anything else failing.
    #[test]
    fn the_working_space_neutral_is_d65() {
        let w = apply(&PROPHOTO_TO_XYZ, [1.0, 1.0, 1.0]);
        let s: f32 = w.iter().sum();
        let (x, y) = (w[0] / s, w[1] / s);
        assert!((x - 0.31272).abs() < 0.001, "D65 x was {x}");
        assert!((y - 0.32903).abs() < 0.001, "D65 y was {y}");
    }

    /// It is the composition, not a fresh derivation — so if either half is ever re-derived this
    /// fails rather than drifting silently.
    #[test]
    fn prophoto_to_xyz_is_the_composition_of_its_halves() {
        for c in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.3, 0.6, 0.2],
        ] {
            let direct = apply(&PROPHOTO_TO_XYZ, c);
            let via = apply(&SRGB_TO_XYZ, apply(&PROPHOTO_TO_SRGB, c));
            for i in 0..3 {
                assert!(
                    (direct[i] - via[i]).abs() < 1e-6,
                    "{c:?}: {direct:?} vs {via:?}"
                );
            }
        }
    }

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

#[cfg(test)]
mod srgb_xyz_tests {
    use super::*;

    /// ⚠ White is the one input that catches a transposed or mis-scaled matrix, because every row
    /// contributes to it. D65's XYZ is (0.95047, 1.0, 1.08883).
    #[test]
    fn srgb_white_maps_to_d65() {
        let w = apply(&SRGB_TO_XYZ, [1.0, 1.0, 1.0]);
        assert!((w[0] - 0.95047).abs() < 1e-3, "X was {}", w[0]);
        assert!((w[1] - 1.0).abs() < 1e-3, "Y was {}", w[1]);
        assert!((w[2] - 1.08883).abs() < 1e-3, "Z was {}", w[2]);
    }
}
