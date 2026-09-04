//! Color transfer functions. Working space is linear light (sRGB / Rec.709 primaries);
//! output encoding for display/JPEG is the sRGB OETF.
//!
//! (ICC / lcms2-based profile conversions are deferred until camera input profiles or
//! display output profiles are needed; see the Phase 1 plan's Scope note.)

pub mod camera;
pub mod huesatmap;
pub mod oklab;
pub mod output_space;
pub mod spaces;
pub mod wb;
pub use oklab::{linear_srgb_to_oklab, oklab_to_linear_srgb};
pub use output_space::{is_out_of_gamut, OutputSpace};
pub use spaces::{prophoto_to_srgb, srgb_to_prophoto};

/// Encode a linear-light sample to sRGB (the sRGB OETF). Input is clamped to `[0, 1]`.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Decode an sRGB-encoded sample back to linear light. Input is clamped to `[0, 1]`.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear **luminance** (CIE Y) of a linear ProPhoto pixel.
///
/// ⚠ **This is real, physical luminance**, so it is *the same quantity* the Rec.709 weights give
/// for sRGB data — converting a colour between spaces does not change how bright it is. sRGB blue
/// reads 0.0722 through the Rec.709 weights and 0.0606 here; green reads 0.7152 and 0.7169. The
/// small differences are chromatic adaptation, not a change of meaning.
///
/// ⚠⚠ **An earlier design claimed these weights "collapse blue" because the B coefficient is
/// 0.0001, and specified Oklab L instead. That was wrong.** The 0.0001 applies to ProPhoto's *own
/// blue primary* — a far more saturated blue than sRGB's, which genuinely carries almost no
/// luminance. Real blue *content*, expressed in ProPhoto, has ordinary luminance. Generalising
/// from the primary to all blues is the mistake; `luminance_matches_rec709` is the guard that
/// would have caught it.
///
/// ⚠ Because this is linear luminance, every constant tuned against the old Rec.709 luma —
/// `local_contrast`'s `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS` and `GAIN_MAX` — **carries over
/// unchanged**. Swapping in a perceptual lightness would have invalidated all of them.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    PROPHOTO_LUMA[0] * rgb[0] + PROPHOTO_LUMA[1] * rgb[1] + PROPHOTO_LUMA[2] * rgb[2]
}

/// The Y row of ProPhoto→XYZ: the luminance contribution of each primary.
///
/// ⚠ **One definition, replacing the Rec.709 triple that was duplicated across six stage files.**
/// Six copies of a constant that changes with the working space is six chances to update it
/// differently.
pub const PROPHOTO_LUMA: [f32; 3] = [0.2880402, 0.7118741, 0.0000857];

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn srgb_encode_reference_points() {
        assert!(close(linear_to_srgb(0.0), 0.0));
        assert!(close(linear_to_srgb(1.0), 1.0));
        assert!(close(linear_to_srgb(0.5), 0.735_356_9));
        assert!(close(linear_to_srgb(0.003_130_8), 0.040_45));
    }

    #[test]
    fn srgb_roundtrips() {
        for &v in &[0.0f32, 0.02, 0.18, 0.5, 0.9, 1.0] {
            assert!(close(srgb_to_linear(linear_to_srgb(v)), v));
        }
    }

    #[test]
    fn encode_clamps_out_of_range() {
        assert!(close(linear_to_srgb(-0.2), 0.0));
        assert!(close(linear_to_srgb(1.5), 1.0));
    }

    #[test]
    fn luminance_matches_rec709() {
        // ⚠⚠ The test that would have caught the mistake this function's doc comment records.
        // Luminance is physical: taking a colour into ProPhoto and measuring it here must agree
        // with measuring the original through the Rec.709 weights. If it does not, the matrix is
        // wrong — or someone has confused a *primary* with the colours users actually have.
        fn rec709(c: [f32; 3]) -> f32 {
            0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
        }
        for c in [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.35, 0.55, 0.9],
            [0.05, 0.08, 0.35],
            [0.5, 0.5, 0.5],
        ] {
            let direct = rec709(c);
            let via = luminance(spaces::srgb_to_prophoto(c));
            assert!(
                (direct - via).abs() < 0.02,
                "{c:?}: Rec.709 says {direct}, ProPhoto says {via}"
            );
        }
    }

    #[test]
    fn a_neutral_has_luminance_equal_to_its_value() {
        // The weights must sum to 1, or every grey reads brighter or darker than it is and each
        // luminance-weighted stage drifts with it.
        for v in [0.0_f32, 0.18, 0.5, 1.0] {
            let l = luminance([v, v, v]);
            assert!((l - v).abs() < 1e-4, "grey {v} has luminance {l}");
        }
    }

    #[test]
    fn no_channel_is_ignored() {
        // ⚠ Weaker than the old claim on purpose. Blue's coefficient really is tiny, and that is
        // correct — but it must not be zero, or a pure-primary blue would be exactly black and
        // any luminance-ratio stage would divide by it.
        assert!(
            PROPHOTO_LUMA.iter().all(|w| *w > 0.0),
            "a weight is zero or negative"
        );
        assert!(luminance([0.0, 0.0, 1.0]) > 0.0);
    }
}
