//! Oklab — a perceptually uniform color space (Björn Ottosson, 2020).
//!
//! Used by the HSL stage: hue angles are perceptually even here, so band weighting is
//! stable and a lightness change does not drag hue with it. Values may exceed 1.0
//! (scene-referred); Oklab extends fine, and `cbrt` keeps negatives finite.
//!
//! The matrix coefficients are transcribed verbatim from Ottosson's reference values, which
//! carry more digits than `f32` can hold. Keeping them intact makes them checkable against
//! the source; truncating them by hand would only invite transcription errors.
#![allow(clippy::excessive_precision)]

/// Linear sRGB (Rec.709 primaries) → Oklab `[L, a, b]`.
pub fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

    [
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ]
}

/// Oklab `[L, a, b]` → linear sRGB. May return out-of-gamut (negative or >1) values;
/// callers clamp.
pub fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = lab;
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    [
        4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
        -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
        -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
    ]
}

/// Linear ProPhoto (ROMM, D50) → Oklab `[L, a, b]`.
///
/// ⚠ Composed offline as ProPhoto→XYZ(D50) → Bradford → XYZ(D65) → Ottosson's LMS, then fused
/// into one matrix — the same shape as [`linear_srgb_to_oklab`], which fuses sRGB→LMS.
///
/// ⚠ **Deriving sRGB→LMS by this same route does not reproduce Ottosson's published constants
/// exactly** — they agree to about 4.5e-5, because he rounded his primaries differently. Do not
/// "correct" either function to match the other; see `an_srgb_colour_reaches_the_same_oklab`.
pub fn prophoto_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let l = 0.7153451038 * r + 0.3527967327 * g - 0.0682510308 * b;
    let m = 0.2744153967 * r + 0.6678313229 * g + 0.0577801949 * b;
    let s = 0.1098335093 * r + 0.1862950046 * g + 0.7043511321 * b;

    // ⚠ `cbrt`, not `powf(1/3)`: it is sign-preserving, so scene-referred negatives (which a
    // wide working space produces routinely) stay finite instead of becoming NaN.
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

    [
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ]
}

/// Oklab `[L, a, b]` → linear ProPhoto. May return out-of-gamut values; callers clamp.
pub fn oklab_to_prophoto(lab: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = lab;
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    [
        1.7386588588 * l3 - 0.9880931444 * m3 + 0.2495310446 * s3,
        -0.7071491793 * l3 + 1.9343303974 * m3 - 0.2272015198 * s3,
        -0.0840839768 * l3 - 0.3575352425 * m3 + 1.4409285251 * s3,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(rgb: [f32; 3]) -> [f32; 3] {
        oklab_to_linear_srgb(linear_srgb_to_oklab(rgb))
    }

    #[test]
    fn roundtrips_within_tolerance() {
        for rgb in [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.2, 0.5, 0.9],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.5, 1.2, 0.3], // above 1.0: scene-referred highlight
        ] {
            let back = roundtrip(rgb);
            for i in 0..3 {
                assert!(
                    (back[i] - rgb[i]).abs() < 1e-4,
                    "{rgb:?} -> {back:?} (channel {i})"
                );
            }
        }
    }

    #[test]
    fn neutrals_have_no_chroma() {
        for v in [0.05f32, 0.18, 0.5, 1.0] {
            let [_, a, b] = linear_srgb_to_oklab([v, v, v]);
            assert!(a.abs() < 1e-3 && b.abs() < 1e-3, "gray {v} -> a={a} b={b}");
        }
    }

    #[test]
    fn lightness_increases_with_luminance() {
        let dark = linear_srgb_to_oklab([0.05, 0.05, 0.05])[0];
        let mid = linear_srgb_to_oklab([0.25, 0.25, 0.25])[0];
        let light = linear_srgb_to_oklab([0.9, 0.9, 0.9])[0];
        assert!(dark < mid && mid < light, "{dark} {mid} {light}");
    }

    #[test]
    fn primaries_land_on_expected_hue_angles() {
        // Sanity anchors (degrees): these are the canonical Oklab hues of the sRGB primaries.
        let hue = |rgb: [f32; 3]| {
            let [_, a, b] = linear_srgb_to_oklab(rgb);
            let d = b.atan2(a).to_degrees();
            if d < 0.0 {
                d + 360.0
            } else {
                d
            }
        };
        assert!((hue([1.0, 0.0, 0.0]) - 29.23).abs() < 0.5);
        assert!((hue([0.0, 1.0, 0.0]) - 142.50).abs() < 0.5);
        assert!((hue([0.0, 0.0, 1.0]) - 264.05).abs() < 0.5);
    }

    #[test]
    fn prophoto_oklab_round_trips() {
        for c in [
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.8, 0.4, 0.2],
            [2.5, 1.0, 0.1],
        ] {
            let back = oklab_to_prophoto(prophoto_to_oklab(c));
            for i in 0..3 {
                assert!(
                    (back[i] - c[i]).abs() < 1e-4,
                    "{c:?} round-tripped to {back:?}"
                );
            }
        }
    }

    #[test]
    fn an_srgb_colour_reaches_the_same_oklab() {
        // ⚠⚠ The load-bearing test: it checks the *entire* matrix chain against a completely
        // independent path. Converting a colour sRGB→ProPhoto→Oklab must agree with taking it
        // sRGB→Oklab directly, because Oklab is absolute.
        //
        // ⚠ Tolerance 2e-3, not 1e-6, and this is not slack: Ottosson's published sRGB→LMS
        // constants differ from a fresh derivation by ~4.5e-5, so the two paths cannot agree
        // exactly. Tightening this will produce a failure that looks like a broken matrix and
        // is not one.
        for c in [
            [0.5, 0.5, 0.5],
            [0.9, 0.2, 0.1],
            [0.1, 0.6, 0.9],
            [0.3, 0.8, 0.2],
        ] {
            let direct = linear_srgb_to_oklab(c);
            let via = prophoto_to_oklab(crate::spaces::srgb_to_prophoto(c));
            for i in 0..3 {
                assert!(
                    (direct[i] - via[i]).abs() < 2e-3,
                    "{c:?}: direct {direct:?} vs via ProPhoto {via:?}"
                );
            }
        }
    }

    #[test]
    fn grey_has_no_chroma_in_prophoto() {
        // A neutral must land on the L axis. If the matrix rows do not sum to the white point,
        // greys pick up a colour cast that is invisible in a round trip but visible in an image.
        for v in [0.05_f32, 0.18, 0.5, 1.0] {
            let lab = prophoto_to_oklab([v, v, v]);
            assert!(
                lab[1].abs() < 1e-3 && lab[2].abs() < 1e-3,
                "grey {v} has chroma: {lab:?}"
            );
        }
    }
}
