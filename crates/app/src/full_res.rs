//! When to upgrade the editing buffer to a full-resolution decode.
//!
//! Pure and `egui`-free on purpose: the alternative is a rule buried in the frame loop that
//! can only be checked by waiting several seconds for the wrong answer.

use std::time::Duration;

/// How long a session must have been open before its full-resolution decode starts.
///
/// ⚠ This dwell is **not** redundant with the 180 ms open debounce. There is already an
/// implicit ~1.2 s before a `Session` exists (debounce plus a ~1 s half decode), but a
/// `BufferCache` **hit creates a `Session` instantly** — so arrowing back through recently
/// viewed photos would fire a 3 s, 313 MB decode for each one. This is what closes that.
pub const SETTLE: Duration = Duration::from_millis(800);

/// Whether to start a full-resolution decode now.
///
/// ⚠ `is_raw` is not a detail. JPEG, PNG and TIFF are already decoded at full size, so their
/// view is already honest and an "upgrade" would spend seconds re-decoding the same pixels.
pub fn wants_upgrade(is_raw: bool, upgraded: bool, in_flight: bool, age: Duration) -> bool {
    is_raw && !upgraded && !in_flight && age >= SETTLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_raw_wants_an_upgrade() {
        // A full decode of a JPEG would produce the buffer we already have.
        assert!(wants_upgrade(true, false, false, SETTLE));
        assert!(!wants_upgrade(false, false, false, SETTLE));
    }

    #[test]
    fn the_dwell_gates_the_upgrade() {
        // Just under the dwell, nothing happens; at it, the decode starts. The boundary is
        // inclusive so a timer that lands exactly on it is not left waiting a whole frame.
        assert!(!wants_upgrade(true, false, false, Duration::ZERO));
        assert!(!wants_upgrade(
            true,
            false,
            false,
            SETTLE - Duration::from_millis(1)
        ));
        assert!(wants_upgrade(true, false, false, SETTLE));
        assert!(wants_upgrade(
            true,
            false,
            false,
            SETTLE + Duration::from_secs(30)
        ));
    }

    #[test]
    fn an_upgrade_is_not_requested_twice() {
        let old = SETTLE + Duration::from_secs(5);
        // Neither while one is in flight...
        assert!(!wants_upgrade(true, false, true, old));
        // ...nor once one has landed. Without this the frame loop would fire a fresh 3 s
        // decode every frame for as long as the photo stayed open.
        assert!(!wants_upgrade(true, true, false, old));
    }
}
