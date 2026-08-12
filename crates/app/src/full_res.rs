//! When to upgrade the editing buffer to a full-resolution decode, and which long edge the
//! native tiers render at.
//!
//! Pure and `egui`-free on purpose: the alternative is a rule buried in the frame loop that
//! can only be checked by zooming in and waiting three seconds for the wrong answer.

use crate::view_region::RenderTier;

/// Whether to start a full-resolution decode now.
///
/// `wanted` is the tier the current zoom calls for — [`crate::view_region::tier_for`]'s answer
/// *before* clamping, since a clamped tier could never ask for a buffer that does not exist.
///
/// ⚠ `is_raw` is not a detail. JPEG, PNG and TIFF are already decoded at full size with
/// `source_scale` 1.0, so their 1:1 view is already honest and an "upgrade" would spend ~3 s
/// re-decoding the very same pixels.
pub fn wants_upgrade(is_raw: bool, wanted: RenderTier, has_full: bool, in_flight: bool) -> bool {
    is_raw && wanted == RenderTier::NativeFull && !has_full && !in_flight
}

/// The long edge to render a native tier at: the full-resolution buffer's when one has been
/// decoded, the editing buffer's otherwise.
///
/// An absent or failed full decode therefore degrades to exactly the previous behaviour.
pub fn native_long(buffer_long: usize, full_long: Option<usize>) -> usize {
    full_long.unwrap_or(buffer_long)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_raw_wants_an_upgrade() {
        // A full decode of a JPEG would produce the buffer we already have.
        assert!(wants_upgrade(true, RenderTier::NativeFull, false, false));
        assert!(!wants_upgrade(false, RenderTier::NativeFull, false, false));
    }

    #[test]
    fn only_the_top_tier_wants_an_upgrade() {
        // Below NativeFull the half-size buffer still has every pixel the screen can show, so
        // spending 3 s and 313 MB would buy nothing.
        for tier in [RenderTier::Fit, RenderTier::NativeHalf] {
            assert!(!wants_upgrade(true, tier, false, false), "{tier:?}");
        }
    }

    #[test]
    fn an_upgrade_is_not_requested_twice() {
        // Neither while one is in flight...
        assert!(!wants_upgrade(true, RenderTier::NativeFull, false, true));
        // ...nor once one has landed. Without this the frame loop would fire a fresh 3 s
        // decode every frame for as long as you stayed zoomed in.
        assert!(!wants_upgrade(true, RenderTier::NativeFull, true, false));
    }

    #[test]
    fn an_absent_full_buffer_falls_back_to_native_half() {
        assert_eq!(native_long(3123, None), 3123);
        assert_eq!(native_long(3123, Some(6246)), 6246);
    }
}
