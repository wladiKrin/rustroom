//! How a Library grid cell is laid out (2026-09-29, the author's calls, four rounds).
//!
//! ⚠ The picture box is a **fixed 3:2**, whatever the photographs' shapes — the author's call
//! after three tries: a square box stretched every landscape cell, a box that followed the
//! majority orientation jumped as thumbnails loaded. A portrait frame is simply smaller inside
//! the box; that is fine. The cell is exactly the box, the caption and the marks row, and the
//! picture sits on the box's bottom edge with the name right under it.

/// One grid cell's geometry, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    pub box_w: f32,
    pub box_h: f32,
    /// The cell's height: insets, the box, the caption, the marks row, and the spacing between.
    pub height: f32,
}

pub const INSET: f32 = 6.0;
/// The picture box's aspect, width over height: the 3:2 frame.
pub const BOX_ASPECT: f32 = 1.5;

pub fn grid_cell(cell: f32, caption_h: f32, marks_h: f32, spacing_y: f32) -> GridCell {
    let box_w = (cell - 2.0 * INSET).max(1.0);
    let box_h = (box_w / BOX_ASPECT).max(1.0);
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
    fn the_box_is_three_by_two_and_the_cell_is_exactly_its_content() {
        let c = grid_cell(224.0, 14.0, 20.0, 4.0);
        assert_eq!(c.box_w, 212.0);
        assert!((c.box_h - 212.0 / 1.5).abs() < 1e-4);
        assert_eq!(c.height, 12.0 + c.box_h + 4.0 + 14.0 + 4.0 + 20.0);
        assert!(c.height < 224.0, "shorter than the old square cell");
    }

    #[test]
    fn a_tiny_cell_never_loses_its_box() {
        let c = grid_cell(8.0, 30.0, 30.0, 4.0);
        assert!(c.box_w >= 1.0 && c.box_h >= 1.0);
    }
}
