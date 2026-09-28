//! The clipping overlay's pixels: red where a highlight has clipped, blue where a shadow has,
//! transparent everywhere else — Lightroom's `J`, computed from the displayed frame.
//!
//! ⚠ A lib module: the rule is a per-pixel threshold on 8-bit display values, testable on a
//! handful of bytes. The bin only turns the result into a texture and paints it over the
//! preview at the preview's own rectangle.

/// Premultiplied RGBA, one pixel per input pixel, for a frame of `w × h` pixels laid out
/// `stride` bytes apart (3 for RGB, 4 for RGBA).
///
/// Highlight clipping is **any** channel at the top; shadow clipping is **every** channel at
/// the bottom — a saturated red at 255 has lost detail in red even while green and blue have
/// room, but a dark blue with red and green at 0 is a colour, not a lost shadow.
pub fn overlay(bytes: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
    const HIGH: [u8; 4] = [255, 40, 40, 255];
    const LOW: [u8; 4] = [40, 90, 255, 255];
    const NONE: [u8; 4] = [0, 0, 0, 0];
    let mut out = Vec::with_capacity(w * h * 4);
    for px in bytes.chunks_exact(stride).take(w * h) {
        let (r, g, b) = (px[0], px[1], px[2]);
        let colour = if r >= 254 || g >= 254 || b >= 254 {
            HIGH
        } else if r <= 1 && g <= 1 && b <= 1 {
            LOW
        } else {
            NONE
        };
        out.extend_from_slice(&colour);
    }
    out
}

/// Whether an overlay has anything to show — so the caller can skip the texture.
pub fn is_empty(overlay: &[u8]) -> bool {
    overlay.as_chunks::<4>().0.iter().all(|p| p[3] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_go_red_shadows_go_blue_and_the_rest_stays_clear() {
        // Four RGB pixels: clipped white, clipped in red only, black, and a mid grey.
        let bytes = [255, 255, 255, 255, 10, 10, 0, 0, 0, 128, 128, 128];
        let o = overlay(&bytes, 4, 1, 3);
        assert_eq!(o.len(), 16);
        assert_eq!(
            &o[0..4],
            &[255, 40, 40, 255],
            "white is a clipped highlight"
        );
        assert_eq!(
            &o[4..8],
            &[255, 40, 40, 255],
            "one clipped channel is enough"
        );
        assert_eq!(&o[8..12], &[40, 90, 255, 255], "black is a clipped shadow");
        assert_eq!(&o[12..16], &[0, 0, 0, 0], "grey is nothing");
        assert!(!is_empty(&o));
    }

    #[test]
    fn a_dark_colour_is_not_a_clipped_shadow() {
        let o = overlay(&[0, 0, 60, 128, 128, 128], 2, 1, 3);
        assert!(is_empty(&o));
    }

    #[test]
    fn rgba_input_is_read_with_its_stride() {
        let o = overlay(&[255, 255, 255, 255, 128, 128, 128, 255], 2, 1, 4);
        assert_eq!(&o[0..4], &[255, 40, 40, 255]);
        assert_eq!(&o[4..8], &[0, 0, 0, 0]);
    }
}
