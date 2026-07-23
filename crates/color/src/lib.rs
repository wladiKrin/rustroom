//! Color transfer functions. Working space is linear light (sRGB / Rec.709 primaries);
//! output encoding for display/JPEG is the sRGB OETF.
//!
//! (ICC / lcms2-based profile conversions are deferred until camera input profiles or
//! display output profiles are needed; see the Phase 1 plan's Scope note.)

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
}
