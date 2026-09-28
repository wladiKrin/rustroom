//! When Develop shows a preview instead of the live render (M9.14): only where the live
//! buffer cannot supply the pixels the canvas asked for **and** the preview can supply more.
//!
//! Pure and `egui`-free on purpose, `full_res::wants_upgrade`'s reason: the alternative is a
//! rule buried in the canvas that can only be checked by zooming and squinting.

/// `wanted_long` is the rung the canvas asked for (`view_region::tier_max_dim`), `buffer_long`
/// the editing buffer's long edge, `preview_long` the preview texture's. At fit the half-size
/// buffer has the pixels; at 1:1 before the full decode it does not and a 71% camera JPEG is
/// nearer; once the full decode has landed the buffer wins everywhere.
pub fn prefer_preview(wanted_long: usize, buffer_long: usize, preview_long: usize) -> bool {
    wanted_long > buffer_long && preview_long > buffer_long
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_stands_in_only_where_the_buffer_falls_short_and_the_preview_does_not() {
        // (wanted, buffer, preview) → show the preview?  X-T4 numbers: 6240 full, 3120 half,
        // 4416 the camera JPEG.
        let table = [
            (
                1600,
                3120,
                4416,
                false,
                "fit: the half-size buffer has the pixels",
            ),
            (3120, 3120, 4416, false, "50%: still the buffer's"),
            (
                4416,
                3120,
                4416,
                true,
                "71%: the buffer falls short, the camera JPEG covers it",
            ),
            (
                6240,
                3120,
                4416,
                true,
                "1:1: the preview is nearer than the buffer",
            ),
            (
                6240,
                6240,
                4416,
                false,
                "1:1 after the full decode landed: the buffer wins",
            ),
            (
                6240,
                3120,
                3120,
                false,
                "a preview no larger than the buffer adds nothing",
            ),
            (6240, 3120, 0, false, "no preview"),
        ];
        for (wanted, buffer, preview, want, why) in table {
            assert_eq!(prefer_preview(wanted, buffer, preview), want, "{why}");
        }
    }
}
