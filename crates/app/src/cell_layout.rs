//! How much of a Library grid cell the picture gets (2026-09-29).
//!
//! ⚠ The picture box used to be a landscape strip two thirds of the cell tall, whatever the
//! photograph's shape — a portrait frame used under half of the cell's width, and the author
//! asked why the boxes took so much space and the pictures so little of it. The box now takes
//! the cell's height minus what the caption and the marks row need, so a landscape frame fills
//! the width and a portrait frame the height.

/// The picture box for a `cell`-sized square cell, given the caption's line height, the marks
/// row's height and the vertical spacing between the three. Never narrower than the cell's
/// inset width, never shorter than half of it.
pub fn thumb_box(cell: f32, caption_h: f32, marks_h: f32, spacing_y: f32) -> (f32, f32) {
    let w = (cell - 12.0).max(1.0);
    let h = (cell - 12.0 - caption_h - marks_h - 2.0 * spacing_y).max(w * 0.5);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_takes_the_height_the_caption_and_the_marks_leave() {
        let (w, h) = thumb_box(224.0, 14.0, 20.0, 4.0);
        assert_eq!(w, 212.0);
        assert_eq!(h, 224.0 - 12.0 - 14.0 - 20.0 - 8.0);
        assert!(
            h > w * 0.66,
            "the box must be taller than the old two-thirds strip: {h} vs {}",
            w * 0.66
        );
        // A portrait 2:3 frame in that box is bounded by the height, and lands wider than it
        // did in the strip.
        let portrait_w = |box_h: f32| box_h * 2.0 / 3.0;
        assert!(portrait_w(h) > portrait_w(w * 0.66) * 1.2);
    }

    #[test]
    fn a_tiny_cell_keeps_half_its_width_for_the_picture() {
        let (w, h) = thumb_box(40.0, 30.0, 30.0, 4.0);
        assert_eq!(w, 28.0);
        assert_eq!(h, 14.0, "never below half the width");
    }
}
