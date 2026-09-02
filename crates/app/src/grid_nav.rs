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

/// The vertical scroll offset that puts the row holding `pos` in the middle of the viewport.
///
/// ⚠⚠ **The clamp is what gives "unless it is near an edge" for free.** Asking for the first row
/// to be centred would scroll to a negative offset and asking for the last would scroll past the
/// end; clamping to the real range leaves those rows where they are and centres everything
/// between, which is exactly the behaviour wanted and needs no special case.
///
/// ⚠ `row_pitch` is the row height **plus** the spacing egui adds between rows — `show_rows`
/// adds it itself, so a pitch of the bare cell height drifts further from the truth with every
/// row and puts the wrong photo in the middle near the bottom of a long grid.
pub fn centre_offset(pos: usize, cols: usize, row_pitch: f32, viewport_h: f32, rows: usize) -> f32 {
    if !row_pitch.is_finite() || row_pitch <= 0.0 || !viewport_h.is_finite() {
        return 0.0;
    }
    let row = pos / cols.max(1);
    let content = rows as f32 * row_pitch;
    let want = row as f32 * row_pitch + row_pitch * 0.5 - viewport_h * 0.5;
    want.clamp(0.0, (content - viewport_h).max(0.0))
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

    /// ⚠⚠ The middle rows centre, and this is the property the whole feature is.
    #[test]
    fn a_middle_row_is_centred() {
        // 20 rows of 100, a 400-tall viewport. Row 10 spans 1000..1100, centre 1050; the
        // viewport should therefore start at 1050 - 200 = 850.
        assert_eq!(centre_offset(10 * 3, 3, 100.0, 400.0, 20), 850.0);
    }

    /// ⚠⚠ **Near the edges it does not centre, and that falls out of the clamp rather than being
    /// a case.** Centring the first row wants a negative offset and centring the last wants one
    /// past the end; both are impossible, so those rows stay where they are.
    #[test]
    fn the_first_and_last_rows_stay_put() {
        assert_eq!(centre_offset(0, 3, 100.0, 400.0, 20), 0.0);
        assert_eq!(
            centre_offset(1, 3, 100.0, 400.0, 20),
            0.0,
            "row 0 is still row 0"
        );
        // Content is 2000 tall in a 400 viewport, so the furthest it can scroll is 1600.
        assert_eq!(centre_offset(19 * 3, 3, 100.0, 400.0, 20), 1600.0);
    }

    /// A grid shorter than its viewport cannot scroll at all.
    #[test]
    fn a_short_grid_never_scrolls() {
        assert_eq!(centre_offset(0, 3, 100.0, 400.0, 2), 0.0);
        assert_eq!(centre_offset(5, 3, 100.0, 400.0, 2), 0.0);
    }

    /// ⚠ A degenerate pitch or viewport must not produce a NaN offset — egui would then scroll
    /// nowhere at all, and a NaN compares false against every clamp so it survives one.
    #[test]
    fn degenerate_inputs_give_a_usable_offset() {
        assert_eq!(centre_offset(5, 3, 0.0, 400.0, 20), 0.0);
        assert_eq!(centre_offset(5, 3, f32::NAN, 400.0, 20), 0.0);
        // ⚠ Zero columns is *repaired* to one rather than refused — `cols.max(1)` — so this
        // centres position 5 as row 5 rather than returning nothing. Asserting 0.0 here was
        // wrong about correct code: a repair that produces a usable answer is the point.
        assert_eq!(centre_offset(5, 0, 100.0, 400.0, 20), 350.0);
    }
}
