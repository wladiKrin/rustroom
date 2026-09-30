//! The rubber band: which cells a drag across the grid catches.

use eframe::egui;

/// The entry indices whose cells the rubber band touches.
///
/// ⚠ Pure, so the geometry is testable without a window — the `thumb_window` precedent. The rects
/// come from `Response::rect` and are therefore screen-space, which is also what the band is.
///
/// ⚠ **Intersects, not contains.** A band must catch a photo it merely clips, or selecting a run
/// would need the rectangle drawn precisely around every thumbnail.
/// ⚠ Takes the drag's two **corners**, not a rectangle, so the normalisation lives here rather
/// than at the call site. It was in the caller first, which made the "backwards drag" test
/// exercise egui's `from_two_pos` instead of our own use of it — a test that named a bug it could
/// not detect, caught by the sabotage step.
pub(super) fn banded(
    cells: &[(usize, egui::Rect)],
    start: egui::Pos2,
    now: egui::Pos2,
) -> Vec<usize> {
    let band = egui::Rect::from_two_pos(start, now);
    cells
        .iter()
        .filter(|(_, r)| r.intersects(band))
        .map(|(i, _)| *i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of four 100×100 cells at y = 0, 10 px apart.
    fn strip() -> Vec<(usize, egui::Rect)> {
        (0..4)
            .map(|i| {
                let x = i as f32 * 110.0;
                (
                    i,
                    egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(100.0, 100.0)),
                )
            })
            .collect()
    }

    #[test]
    fn a_band_catches_every_cell_it_touches() {
        // ⚠ **Intersects, not contains.** A band must catch a photo it merely clips, or selecting
        // a run would mean drawing the rectangle precisely around every thumbnail.
        let cells = strip();
        // Clips the right edge of cell 0 and the left edge of cell 1.
        assert_eq!(
            banded(&cells, egui::pos2(90.0, 40.0), egui::pos2(120.0, 60.0)),
            vec![0, 1]
        );
    }

    #[test]
    fn a_band_over_empty_space_catches_nothing() {
        let cells = strip();
        assert!(banded(&cells, egui::pos2(0.0, 200.0), egui::pos2(400.0, 260.0)).is_empty());
    }

    #[test]
    fn a_band_drawn_upwards_and_leftwards_works_the_same() {
        // ⚠ `Rect::from_two_pos` normalises, so dragging up-left is the same rectangle as
        // down-right. Without it a backwards drag would select nothing at all.
        let cells = strip();
        let forward = banded(&cells, egui::pos2(50.0, 10.0), egui::pos2(230.0, 90.0));
        let backward = banded(&cells, egui::pos2(230.0, 90.0), egui::pos2(50.0, 10.0));
        assert_eq!(forward, backward);
        assert_eq!(forward, vec![0, 1, 2]);
    }

    #[test]
    fn a_band_covering_everything_selects_everything() {
        let cells = strip();
        assert_eq!(
            banded(&cells, egui::pos2(-10.0, -10.0), egui::pos2(500.0, 200.0)),
            vec![0, 1, 2, 3]
        );
    }
}
