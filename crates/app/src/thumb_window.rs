//! Which items of a uniform strip or grid are on screen, and where to scroll to centre one.
//!
//! ⚠ A grid of rows and a strip of cells are the same one-dimensional problem, so one pair of
//! functions serves both views. The grid's items are **rows** (`step = cell + spacing.y`); the
//! filmstrip's items are **cells** (`step = cell + spacing.x`).
//!
//! ⚠ Deliberately egui-free — plain `f32` and `usize` — following `view_region`, whose own doc
//! says it is kept that way "so it can be tested without a window". Clamping and off-by-one
//! errors are then testable rather than only visible as a wrong image.

use std::ops::Range;

/// Indices of items whose cells intersect the viewport, widened by `margin` items each way.
///
/// `offset` is the scroll position, `extent` the viewport's length along the same axis, `step`
/// one item's pitch (its size plus the gap after it), and `count` how many items exist.
///
/// ⚠ The margin is prefetch, not display: callers draw `visible_span(.., 0)` and *load*
/// `visible_span(.., MARGIN)`.
pub fn visible_span(
    offset: f32,
    extent: f32,
    step: f32,
    count: usize,
    margin: usize,
) -> Range<usize> {
    // ⚠ `!step.is_finite()` comes first and is not redundant: `step <= 0.0` is `false` for `NaN`,
    // so a NaN cell size would sail past a `<=` test alone and produce a saturated, enormous
    // range.
    if count == 0 || !step.is_finite() || step <= 0.0 || !offset.is_finite() || !extent.is_finite()
    {
        return 0..0;
    }
    let first = (offset / step).floor().max(0.0) as usize;
    // Inclusive: the last item whose cell starts before the viewport ends.
    let last = ((offset + extent) / step).floor().max(0.0) as usize;
    let end = last.saturating_add(margin).saturating_add(1).min(count);
    // ⚠ Clamped against `end`, not only against `count`: a stale offset past the content would
    // otherwise give start > end, which is not a range any caller can use.
    let start = first.saturating_sub(margin).min(end);
    start..end
}

/// The scroll offset that centres `index`, clamped so the ends do not scroll past the content.
///
/// ⚠ This replaces `Response::scroll_to_me`, which cannot work on a virtualised list because an
/// off-window cell is never drawn. It is also the correct fix for the drift bug recorded from
/// M2.2: egui computes a `scroll_to_me` target offset **once** and never recomputes it, so the
/// centred photo slid left as thumbnails ahead of it loaded and reflowed the content.
pub fn centre_offset(index: usize, step: f32, extent: f32, content: f32) -> f32 {
    let centre = index as f32 * step + step * 0.5;
    let max = (content - extent).max(0.0);
    (centre - extent * 0.5).clamp(0.0, max)
}

/// Whether a strip's geometry changed since it was last drawn — a resize changes the `step`, a
/// narrower window the `extent` — so its active item must be centred again.
///
/// ⚠⚠ **Reported in use 2026-09-29:** the filmstrip centred the active photo only when the active
/// photo CHANGED, so a resize kept the old offset in points while every cell grew or shrank under
/// it, and the photos slid sideways. `None` — never drawn — counts as a change. The tolerance is a
/// quarter point, far below a resize step and far above float noise.
pub fn layout_changed(before: Option<(f32, f32)>, now: (f32, f32)) -> bool {
    before.is_none_or(|(step, extent)| (step - now.0).abs() > 0.25 || (extent - now.1).abs() > 0.25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resize_or_a_first_draw_is_a_layout_change_and_a_steady_frame_is_not() {
        assert!(layout_changed(None, (150.0, 900.0)), "never drawn");
        assert!(
            layout_changed(Some((150.0, 900.0)), (180.0, 900.0)),
            "taller strip"
        );
        assert!(
            layout_changed(Some((150.0, 900.0)), (150.0, 700.0)),
            "narrower strip"
        );
        assert!(
            !layout_changed(Some((150.0, 900.0)), (150.0, 900.0)),
            "a steady frame"
        );
        assert!(
            !layout_changed(Some((150.0, 900.0)), (150.1, 900.0)),
            "float noise"
        );
    }

    // Items are 40 long; the viewport shows 100 of them.
    const STEP: f32 = 40.0;
    const EXTENT: f32 = 100.0;

    #[test]
    fn the_top_of_the_list_starts_at_zero() {
        // Items 0, 1 and 2 intersect [0, 100).
        assert_eq!(visible_span(0.0, EXTENT, STEP, 100, 0), 0..3);
    }

    #[test]
    fn a_scrolled_viewport_starts_at_the_first_intersecting_item() {
        // ⚠ The *first intersecting* item, not the first fully visible one. At offset 100 item 2
        // spans 80..120, so it is half on screen — drawing from item 3 would leave a gap at the
        // top that looks like a rendering bug.
        assert_eq!(visible_span(100.0, EXTENT, STEP, 100, 0), 2..6);
    }

    #[test]
    fn the_margin_widens_both_ends() {
        assert_eq!(visible_span(400.0, EXTENT, STEP, 100, 2), 8..15);
    }

    #[test]
    fn the_margin_is_clamped_at_both_ends() {
        // ⚠ Both ends, in one test, because a `saturating_sub` at the start and a `min` at the end
        // are different mistakes and each is easy to make alone.
        assert_eq!(visible_span(0.0, EXTENT, STEP, 100, 5).start, 0);
        assert_eq!(visible_span(3900.0, EXTENT, STEP, 100, 5).end, 100);
    }

    #[test]
    fn an_offset_past_the_content_yields_an_empty_range() {
        // A stale scroll offset after the list shrinks must not produce start > end, which would
        // panic anything treating the range as a length.
        let span = visible_span(9000.0, EXTENT, STEP, 100, 0);
        assert!(span.start <= span.end, "{span:?}");
        assert!(span.is_empty(), "{span:?}");
    }

    #[test]
    fn an_empty_list_yields_an_empty_range() {
        assert_eq!(visible_span(0.0, EXTENT, STEP, 0, 4), 0..0);
    }

    #[test]
    fn a_degenerate_step_yields_an_empty_range() {
        // ⚠ A zero step means a zero-height cell — division would give infinity, and `as usize`
        // saturates rather than panicking, so the failure would be a silently enormous range.
        assert_eq!(visible_span(0.0, EXTENT, 0.0, 100, 0), 0..0);
    }

    #[test]
    fn a_middle_item_is_centred() {
        // Item 10 spans 400..440, centre 420; the viewport is 100 long, so it starts at 370.
        assert_eq!(centre_offset(10, STEP, EXTENT, 4000.0), 370.0);
    }

    #[test]
    fn centring_the_first_item_does_not_scroll_past_the_start() {
        assert_eq!(centre_offset(0, STEP, EXTENT, 4000.0), 0.0);
    }

    #[test]
    fn centring_the_last_item_does_not_scroll_past_the_end() {
        // ⚠ This is the half `scroll_to_me` used to get right for free. Without the upper clamp
        // the strip scrolls into blank space past the final thumbnail.
        assert_eq!(centre_offset(99, STEP, EXTENT, 4000.0), 3900.0);
    }

    #[test]
    fn content_shorter_than_the_viewport_never_scrolls() {
        assert_eq!(centre_offset(1, STEP, EXTENT, 80.0), 0.0);
    }
}
