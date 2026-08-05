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
}
