//! How a Library grid cell is laid out (2026-09-29, the author's two calls).
//!
//! ⚠ The picture box used to be a landscape strip two thirds of the cell tall, whatever the
//! photograph's shape, and the cell was square whatever its content — a portrait frame used
//! under half of the cell's width, and the caption sat well above the cell's bottom edge. Now
//! the box is **square** (a landscape frame fills its width, a portrait frame its height) and
//! the cell is exactly as tall as the box, the caption and the marks row, so nothing is left
//! over below the name.

/// One grid cell's geometry, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    /// The picture box, square: the cell's width less the 6-pt insets each side.
    pub box_side: f32,
    /// The cell's height: insets, the box, the caption, the marks row, and the spacing between.
    pub height: f32,
}

pub const INSET: f32 = 6.0;

pub fn grid_cell(cell: f32, caption_h: f32, marks_h: f32, spacing_y: f32) -> GridCell {
    let box_side = (cell - 2.0 * INSET).max(1.0);
    GridCell {
        box_side,
        height: 2.0 * INSET + box_side + spacing_y + caption_h + spacing_y + marks_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_is_square_and_the_cell_is_exactly_its_content() {
        let c = grid_cell(224.0, 14.0, 20.0, 4.0);
        assert_eq!(c.box_side, 212.0);
        assert_eq!(c.height, 12.0 + 212.0 + 4.0 + 14.0 + 4.0 + 20.0);
        assert!(
            c.height > 224.0,
            "taller than square, by the caption and the marks"
        );
    }

    #[test]
    fn a_tiny_cell_never_loses_its_box() {
        let c = grid_cell(8.0, 30.0, 30.0, 4.0);
        assert_eq!(c.box_side, 1.0);
    }
}
