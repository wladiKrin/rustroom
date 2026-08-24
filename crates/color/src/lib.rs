//! Color transfer functions. Working space is linear light (sRGB / Rec.709 primaries);
//! output encoding for display/JPEG is the sRGB OETF.
//!
//! (ICC / lcms2-based profile conversions are deferred until camera input profiles or
//! display output profiles are needed; see the Phase 1 plan's Scope note.)

pub mod oklab;
pub mod spaces;
pub use oklab::{linear_srgb_to_oklab, oklab_to_linear_srgb};
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

/// Perceptual lightness of a linear **ProPhoto** pixel — Oklab's L.
///
/// ⚠⚠ **This exists because ProPhoto's own luminance coefficients are unusable.** The ROMM→XYZ
/// luminance row is `R 0.2880, G 0.7119, B 0.0001` — blue contributes essentially nothing,
/// because ProPhoto's blue primary sits at y = 0.0001 on the chromaticity diagram. A stage using
/// that as "luma" would stop seeing blue detail entirely: sharpening would leave a blue iris or a
/// sky gradient untouched, and dehaze estimates airlight from brightness while haze is blue.
///
/// Oklab L is defined from XYZ, so it is working-space independent and does not collapse any
/// channel. `every_channel_contributes_to_lightness` is the guard.
///
/// ⚠ **L is a cube root, not linear luminance.** Constants tuned against linear luma do not
/// carry over — see [`MID_GREY_L`] and the spec's "five tuned constants" section.
pub fn lightness(rgb: [f32; 3]) -> f32 {
    oklab::prophoto_to_oklab(rgb)[0]
}

/// The lightness of mid-grey — linear 0.18 — in [`lightness`]'s units.
///
/// ⚠ **0.5646, not 0.18.** Any stage weighting midtones must use this: `local_contrast`'s
/// midtone weight is `t = l / (l + 0.18)`, and left at 0.18 against Oklab L it peaks a shade
/// above black, applying Clarity's midtone protection to the *shadows* instead. That is close to
/// the inverse of its purpose and produces a plausible-looking image, which is what makes it the
/// dangerous one.
pub const MID_GREY_L: f32 = 0.5646;

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
    fn every_channel_contributes_to_lightness() {
        // ⚠⚠ The regression guard for the finding this whole design rests on. ProPhoto's own
        // luminance weights give blue 0.0001, so a "simplification" back to working-space luma
        // would make pure blue almost black — and nothing else in the suite would notice.
        let black = lightness([0.0, 0.0, 0.0]);
        for (name, c) in [
            ("red", [1.0, 0.0, 0.0]),
            ("green", [0.0, 1.0, 0.0]),
            ("blue", [0.0, 0.0, 1.0]),
        ] {
            let l = lightness(c);
            assert!(
                l > black + 0.2,
                "{name} has lightness {l}, barely above black {black} — \
                 this is what working-space luma does to blue"
            );
        }
    }

    #[test]
    fn mid_grey_l_matches_the_function() {
        // ⚠ The constant and the function must not drift. A hand-written constant that no test
        // ties to its source is one that silently goes stale the day the space changes.
        let measured = lightness([0.18, 0.18, 0.18]);
        assert!(
            (measured - MID_GREY_L).abs() < 1e-3,
            "MID_GREY_L is {MID_GREY_L} but lightness(0.18) is {measured}"
        );
    }

    #[test]
    fn lightness_is_monotonic_and_compresses_the_shadows() {
        // ⚠ Pins the *character* change, not just the values: L is a cube root, so it rises
        // steeply out of black. A stage guard tuned for linear luma — `LUMA_EPS = 1e-3` in
        // sharpening — fires far higher up the range here, which is why the spec re-derives it.
        let a = lightness([0.001, 0.001, 0.001]);
        let b = lightness([0.01, 0.01, 0.01]);
        let c = lightness([0.5, 0.5, 0.5]);
        assert!(a < b && b < c, "lightness must be monotonic");
        assert!(
            a > 0.05,
            "linear 0.001 should already be well off the floor in L, got {a}"
        );
    }
}
