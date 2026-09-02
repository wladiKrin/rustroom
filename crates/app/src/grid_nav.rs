//! Moving through a virtualised grid with the arrow keys, and keeping what you land on in view.
//!
//! ⚠ Pure arithmetic, deliberately: the Library grid and Survey both need it, and a scroll
//! position worked out inline in two places is how the two come to disagree about where a photo
//! is. Neither function has ever seen an `egui::Ui`.

/// The visible index `delta` steps away from `cur`, clamped at both ends.
///
/// ⚠ `delta` is `±1` for a horizontal step and `±cols` for a vertical one — the same function
/// serves both, because a grid *is* its visible list read in rows.
///
/// ⚠ **Clamped, never wrapping.** Wrapping from the last row to the first is disorienting in a
/// grid you are scrolling, and it is the rule the arrows already followed left-to-right.
///
/// ⚠ When `cur` is not itself visible — it was filtered away under you — the nearest visible
/// index below it stands in, so an arrow still moves rather than doing nothing.
pub fn step_by(visible: &[usize], cur: usize, delta: i64) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    let last = visible.len() as i64 - 1;
    let target = match visible.binary_search(&cur) {
        Ok(p) => p as i64 + delta,
        // ⚠⚠ `cur` is **hidden**: `p` counts the visible indices below it, so `p` is *already*
        // the next one above and `p - 1` the previous one below. A forward step therefore needs
        // one fewer than it looks — treating `p` as the current position would skip the very
        // photo the arrow should land on. `format_filter::step`, which this generalises, records
        // the same asymmetry, and matching it is what keeps the grid and the filmstrip agreeing
        // about where an arrow goes.
        Err(p) if delta > 0 => p as i64 + delta - 1,
        Err(p) => p as i64 + delta,
    };
    Some(visible[target.clamp(0, last) as usize])
}

/// How far into the viewport a followed row is allowed to sit before the view moves, as a
/// fraction of the viewport's height.
///
/// ⚠⚠ **A band, not a point, and the difference is the whole feel of the thing.** Centring the
/// selection on every keypress makes the grid lurch under you — reported as feeling "very
/// abrupt", and it is: the picture moves even when what you selected was already comfortably in
/// view. Inside the band nothing scrolls at all; outside it, the view moves just far enough to
/// bring the row back to the near edge of the band. That is the behaviour every file browser and
/// text editor has, and nobody notices it, which is the point.
pub const FOLLOW_BAND: (f32, f32) = (0.25, 0.75);

/// The scroll offset that keeps the row holding `pos` inside [`FOLLOW_BAND`], or `None` when it
/// is already there and the view should not move at all.
///
/// ⚠ Returning `None` rather than the current offset matters: forcing `vertical_scroll_offset`
/// even to the value it already has pins the scroll area for that frame, which fights any
/// smooth scrolling in flight.
///
/// ⚠ Clamped to the real scroll range, which is what makes the first and last rows behave: the
/// view simply cannot move further, so they sit wherever they fall.
///
/// ⚠ `row_pitch` is the row height **plus** the spacing egui adds between rows — `show_rows`
/// adds it itself, so a pitch of the bare cell height drifts further from the truth with every
/// row and moves the view at the wrong moment near the bottom of a long grid.
pub fn follow_offset(
    pos: usize,
    cols: usize,
    row_pitch: f32,
    viewport_h: f32,
    rows: usize,
    current: f32,
) -> Option<f32> {
    if !row_pitch.is_finite() || row_pitch <= 0.0 || !viewport_h.is_finite() || viewport_h <= 0.0 {
        return None;
    }
    let current = if current.is_finite() { current } else { 0.0 };
    let max = (rows as f32 * row_pitch - viewport_h).max(0.0);
    let row = pos / cols.max(1);
    let top = row as f32 * row_pitch;
    let bottom = top + row_pitch;
    let (lo, hi) = (
        current + viewport_h * FOLLOW_BAND.0,
        current + viewport_h * FOLLOW_BAND.1,
    );
    let want = if top < lo {
        top - viewport_h * FOLLOW_BAND.0
    } else if bottom > hi {
        bottom - viewport_h * FOLLOW_BAND.1
    } else {
        return None;
    };
    let want = want.clamp(0.0, max);
    // ⚠ The clamp can land back on where we already are — at the top and bottom of the grid it
    // always does — and moving to where you already were is not a move.
    (want != current).then_some(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dense visible list, the ordinary case.
    const DENSE: [usize; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    #[test]
    fn a_horizontal_step_moves_by_one() {
        assert_eq!(step_by(&DENSE, 4, 1), Some(5));
        assert_eq!(step_by(&DENSE, 4, -1), Some(3));
    }

    /// ⚠ A vertical step is a step of `cols` through the *visible* list, which is what makes one
    /// function serve both axes.
    #[test]
    fn a_vertical_step_moves_by_a_row() {
        assert_eq!(step_by(&DENSE, 1, 4), Some(5));
        assert_eq!(step_by(&DENSE, 9, -4), Some(5));
    }

    /// ⚠ Clamped at both ends, never wrapping.
    #[test]
    fn the_ends_clamp_rather_than_wrapping() {
        assert_eq!(step_by(&DENSE, 0, -1), Some(0));
        assert_eq!(step_by(&DENSE, 0, -4), Some(0));
        assert_eq!(step_by(&DENSE, 9, 1), Some(9));
        assert_eq!(step_by(&DENSE, 9, 4), Some(9));
    }

    /// ⚠⚠ **A sparse list is the case this project has got wrong three times.** The step is
    /// through *positions* in the visible list, not through entry indices — moving by `cols`
    /// entry indices would skip a different number of tiles per row on a filtered grid.
    #[test]
    fn a_sparse_list_steps_by_position_not_by_index() {
        let sparse = [2usize, 5, 9, 30, 44, 61];
        assert_eq!(step_by(&sparse, 5, 1), Some(9), "one step is one tile");
        assert_eq!(
            step_by(&sparse, 2, 3),
            Some(30),
            "three steps is three tiles"
        );
    }

    /// ⚠⚠ **A photo filtered away under the cursor must not make a forward arrow skip one.**
    /// `binary_search`'s `Err(p)` is already the position *above* `cur`, so a naive `p + delta`
    /// lands one too far — and the grid would then disagree with the filmstrip about where `→`
    /// goes from the same starting point.
    #[test]
    fn a_hidden_current_index_steps_to_its_neighbour_not_past_it() {
        let sparse = [2usize, 5, 9];
        assert_eq!(step_by(&sparse, 7, 1), Some(9), "forward skipped a photo");
        assert_eq!(step_by(&sparse, 7, -1), Some(5));
    }

    /// ⚠ And it agrees with the one-dimensional stepper the filmstrip uses, at every index and
    /// in both directions — the property that makes generalising it safe rather than a second
    /// rule that happens to look similar.
    #[test]
    fn a_single_step_matches_the_filmstrips_stepper() {
        let sparse = [2usize, 5, 9, 30];
        for cur in 0..40 {
            for (delta, forward) in [(1i64, true), (-1, false)] {
                assert_eq!(
                    step_by(&sparse, cur, delta),
                    crate::format_filter::step(&sparse, cur, forward),
                    "disagreed at {cur} going {}",
                    if forward { "forward" } else { "back" }
                );
            }
        }
    }

    #[test]
    fn an_empty_list_steps_nowhere() {
        assert_eq!(step_by(&[], 0, 1), None);
    }

    /// 20 rows of 100 in a 400-tall viewport: the band runs from `offset + 100` to
    /// `offset + 300`.
    fn follow(row: usize, current: f32) -> Option<f32> {
        follow_offset(row * 3, 3, 100.0, 400.0, 20, current)
    }

    /// ⚠⚠ **Inside the band, nothing moves.** This is the complaint that put the band here:
    /// centring on every keypress makes the grid lurch even when what you selected was already
    /// comfortably in view.
    #[test]
    fn a_row_already_in_the_band_does_not_scroll() {
        // At offset 500 the band is 600..800, so rows 6, 6.5 and 7 are all inside it.
        assert_eq!(follow(6, 500.0), None);
        assert_eq!(follow(7, 500.0), None);
    }

    /// ⚠ Above the band it moves **just far enough** to reach the band's top edge, not to the
    /// centre — the minimum that puts it back in view.
    #[test]
    fn a_row_above_the_band_scrolls_to_its_top_edge() {
        // At offset 500 the band starts at 600. Row 4 spans 400..500, above it.
        // It should land at 400 - 100 = 300, where the band starts at 400 = the row's top.
        assert_eq!(follow(4, 500.0), Some(300.0));
    }

    /// ⚠ And below the band, to its bottom edge.
    #[test]
    fn a_row_below_the_band_scrolls_to_its_bottom_edge() {
        // At offset 500 the band ends at 800. Row 9 spans 900..1000, below it.
        // It should land at 1000 - 300 = 700, where the band ends at 1000 = the row's bottom.
        assert_eq!(follow(9, 500.0), Some(700.0));
    }

    /// ⚠⚠ **The clamp is what makes the ends behave, and it needs no special case.** The view
    /// cannot scroll above 0 or past `content - viewport`, so the first and last rows simply sit
    /// where they fall — and asking to move to where you already are is not a move.
    #[test]
    fn the_ends_clamp_and_report_no_move_when_already_there() {
        assert_eq!(follow(0, 0.0), None, "already at the top");
        // Content is 2000 in a 400 viewport, so 1600 is as far as it goes.
        assert_eq!(follow(19, 1600.0), None, "already at the bottom");
        assert_eq!(follow(19, 0.0), Some(1600.0), "must scroll to the end");
    }

    /// ⚠ A grid shorter than its viewport cannot scroll at all.
    #[test]
    fn a_short_grid_never_scrolls() {
        assert_eq!(follow_offset(0, 3, 100.0, 400.0, 2, 0.0), None);
        assert_eq!(follow_offset(5, 3, 100.0, 400.0, 2, 0.0), None);
    }

    /// ⚠ A degenerate pitch or viewport must not produce a NaN offset — egui would then scroll
    /// nowhere at all, and a NaN compares false against every clamp so it survives one.
    #[test]
    fn degenerate_inputs_scroll_nowhere() {
        assert_eq!(follow_offset(5, 3, 0.0, 400.0, 20, 0.0), None);
        assert_eq!(follow_offset(5, 3, f32::NAN, 400.0, 20, 0.0), None);
        assert_eq!(follow_offset(5, 3, 100.0, 0.0, 20, 0.0), None);
        // ⚠ A NaN *current* offset is repaired to 0 rather than refused, so the view still
        // follows rather than freezing — a NaN compares false against every clamp and would
        // otherwise survive one.
        // Position 30 is row 10, spanning 1000..1100; from a repaired offset of 0 the band ends
        // at 300, so it lands at 1100 - 300 = 800.
        assert_eq!(
            follow_offset(30, 3, 100.0, 400.0, 20, f32::NAN),
            Some(800.0)
        );
    }
}
