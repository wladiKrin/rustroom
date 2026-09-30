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

/// The rubber band: start, grow, select and draw it — on the outer `ui`, over the cells.
pub(super) fn rubber_band(
    ui: &egui::Ui,
    library: Option<&mut crate::Library>,
    cell_rects: &[(usize, egui::Rect)],
    band_started: Option<egui::Pos2>,
    bg_clicked: bool,
) {
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let released = ui.input(|i| i.pointer.any_released());
    let mods = ui.input(|i| i.modifiers);
    if let Some(lib) = library {
        // ⚠ **Only a plain click**, matching `apply_click`'s rule that only a plain click
        // moves the active photo: a ⌘- or Shift-click on empty space is part of building a
        // selection, not a request to throw it away.
        if bg_clicked && !mods.any() {
            lib.selection.clear();
        }
        if let Some(start) = band_started {
            lib.band = Some((start, start));
        }
        if let Some((start, _)) = lib.band {
            if let Some(now) = pointer {
                lib.band = Some((start, now));
                let rect = egui::Rect::from_two_pos(start, now);
                // (the same normalisation `banded` performs, for the drawn outline)
                // ⚠ Only once the drag is a real drag: a 1 px band on a plain click would
                // replace the selection with the single cell under the cursor, which the click
                // handler is already doing — and would fight it.
                if rect.width() > 3.0 || rect.height() > 3.0 {
                    let hit = banded(cell_rects, start, now);
                    // ⚠ Plain drag replaces; ⌘/Shift adds — the rule `apply_click` follows.
                    //
                    // ⚠ The additive case unions and re-selects rather than toggling: a
                    // toggle would *deselect* a photo the band passed over twice, so sweeping
                    // back and forth would flicker the selection.
                    if mods.command || mods.shift {
                        let mut all: Vec<usize> = lib.selection.iter().collect();
                        for i in hit {
                            if !all.contains(&i) {
                                all.push(i);
                            }
                        }
                        all.sort_unstable();
                        lib.selection.select_indices(&all);
                    } else {
                        lib.selection.select_indices(&hit);
                    }
                    // ⚠ The accent, not a fixed blue: a band is a selection in progress.
                    let accent = ui.visuals().selection.bg_fill;
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(2),
                        egui::Stroke::new(1.0, accent),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        accent.gamma_multiply(0.1),
                    );
                }
            }
        }
        if released {
            lib.band = None;
        }
    }
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
