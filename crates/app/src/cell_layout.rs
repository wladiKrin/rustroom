//! How a Library grid cell is laid out (2026-09-29, the author's calls, three rounds).
//!
//! ⚠ Round one: the picture box was a landscape strip and the cell a square, so the caption
//! floated well above the cell's bottom edge. Round two made the box square with a
//! content-height cell — and every landscape cell grew tall with a gap above the picture.
//! Round three, this: the box takes the **aspect of the majority orientation on screen**
//! (3:2 for a landscape folder, 2:3 for a portrait one), the cell is exactly the box, the
//! caption and the marks row, and the picture sits on the box's bottom edge. The minority
//! orientation is smaller inside that box; no uniform box avoids that, and the majority is what
//! the eye scans.

/// One grid cell's geometry, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    pub box_w: f32,
    pub box_h: f32,
    /// The cell's height: insets, the box, the caption, the marks row, and the spacing between.
    pub height: f32,
}

pub const INSET: f32 = 6.0;
/// The box aspect (width over height) for a landscape majority — the 3:2 frame.
pub const LANDSCAPE: f32 = 1.5;
/// … and for a portrait majority.
pub const PORTRAIT: f32 = 2.0 / 3.0;

/// The box aspect for the pictures on screen: landscape unless portraits outnumber them.
/// `sizes` are the thumbnails already loaded (width, height); with none loaded, landscape.
pub fn majority_aspect(sizes: impl IntoIterator<Item = (f32, f32)>) -> f32 {
    let (mut portrait, mut landscape) = (0usize, 0usize);
    for (w, h) in sizes {
        if h > w {
            portrait += 1;
        } else {
            landscape += 1;
        }
    }
    if portrait > landscape {
        PORTRAIT
    } else {
        LANDSCAPE
    }
}

pub fn grid_cell(cell: f32, aspect: f32, caption_h: f32, marks_h: f32, spacing_y: f32) -> GridCell {
    let box_w = (cell - 2.0 * INSET).max(1.0);
    let box_h = (box_w / aspect.max(0.1)).max(1.0);
    GridCell {
        box_w,
        box_h,
        height: 2.0 * INSET + box_h + spacing_y + caption_h + spacing_y + marks_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_landscape_folder_gets_a_short_cell_and_a_portrait_one_a_tall_cell() {
        let land = grid_cell(224.0, LANDSCAPE, 14.0, 20.0, 4.0);
        assert_eq!(land.box_w, 212.0);
        assert!((land.box_h - 212.0 / 1.5).abs() < 1e-4);
        assert_eq!(land.height, 12.0 + land.box_h + 4.0 + 14.0 + 4.0 + 20.0);
        assert!(
            land.height < 224.0,
            "a landscape cell is shorter than the old square"
        );
        let port = grid_cell(224.0, PORTRAIT, 14.0, 20.0, 4.0);
        assert!((port.box_h - 212.0 * 1.5).abs() < 1e-4);
        assert!(port.height > land.height);
    }

    #[test]
    fn the_majority_orientation_picks_the_box_and_ties_and_nothing_go_landscape() {
        assert_eq!(
            majority_aspect([(30.0, 20.0), (30.0, 20.0), (20.0, 30.0)]),
            LANDSCAPE
        );
        assert_eq!(
            majority_aspect([(20.0, 30.0), (20.0, 30.0), (30.0, 20.0)]),
            PORTRAIT
        );
        assert_eq!(
            majority_aspect([(20.0, 30.0), (30.0, 20.0)]),
            LANDSCAPE,
            "a tie"
        );
        assert_eq!(
            majority_aspect(std::iter::empty()),
            LANDSCAPE,
            "nothing loaded yet"
        );
    }

    #[test]
    fn a_tiny_cell_never_loses_its_box() {
        let c = grid_cell(8.0, LANDSCAPE, 30.0, 30.0, 4.0);
        assert!(c.box_w >= 1.0 && c.box_h >= 1.0);
    }
}
