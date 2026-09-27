//! Pure stroke-assembly maths for the brush.
//!
//! Deliberately free of `egui` types, like `crop_tool` and `mask_tool` beside it: the decimation
//! rule is the one part of painting that can be wrong silently, so it must be testable without a
//! window.

/// The brush tool's starting settings — and what a double-click on each of its sliders restores.
///
/// ⚠ One definition: the app's constructor and the sliders' reset read the same numbers, so a
/// double-click can never land somewhere a fresh start would not.
pub struct BrushDefaults {
    pub size: f32,
    pub hardness: f32,
    pub flow: f32,
    pub tolerance: f32,
}

pub const DEFAULTS: BrushDefaults = BrushDefaults {
    size: 40.0,
    hardness: 50.0,
    flow: 100.0,
    tolerance: 35.0,
};

/// Dabs are splatted a quarter of a radius apart, so recording points more finely than that adds
/// sidecar bytes and nothing else.
pub const SPACING_FRACTION: f32 = 0.25;

/// Whether a new point is far enough from the last recorded one to be worth keeping.
///
/// ⚠ Not tidiness. A drag emits a point every ~16 ms, so a three-second stroke is ~180 points,
/// most of them a pixel apart, and every one is serialised into the sidecar as JSON.
pub fn should_record(last: [f32; 2], next: [f32; 2], radius: f32) -> bool {
    let min = (radius * SPACING_FRACTION).max(1e-5);
    let (dx, dy) = (next[0] - last[0], next[1] - last[1]);
    (dx * dx + dy * dy).sqrt() >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_within_the_spacing_is_dropped() {
        // radius 0.1 -> minimum spacing 0.025.
        assert!(!should_record([0.5, 0.5], [0.51, 0.5], 0.1));
        assert!(!should_record([0.5, 0.5], [0.5, 0.512], 0.1));
    }

    #[test]
    fn a_point_beyond_the_spacing_is_kept() {
        assert!(should_record([0.5, 0.5], [0.53, 0.5], 0.1));
        assert!(should_record([0.5, 0.5], [0.5, 0.6], 0.1));
    }

    #[test]
    fn decimation_scales_with_the_radius() {
        // ⚠ The discriminating case: the SAME offset must be dropped by a fat brush and kept by a
        // thin one. A test using one radius could not tell a radius-scaled rule from a fixed one.
        let off = [0.53, 0.5];
        assert!(
            !should_record([0.5, 0.5], off, 0.5),
            "fat brush should drop"
        );
        assert!(
            should_record([0.5, 0.5], off, 0.02),
            "thin brush should keep"
        );
    }

    #[test]
    fn a_zero_radius_still_terminates() {
        // A degenerate radius must not make the minimum spacing zero and record every event.
        assert!(should_record([0.5, 0.5], [0.5001, 0.5], 0.0));
    }
}
