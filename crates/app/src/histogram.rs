//! Histogram of a display (sRGB, 8-bit RGBA) frame.

/// Per-channel 256-bin counts [R, G, B] from an interleaved RGBA byte buffer.
pub fn histogram_rgba(rgba: &[u8]) -> [[u32; 256]; 3] {
    let mut h = [[0u32; 256]; 3];
    for px in rgba.as_chunks::<4>().0 {
        h[0][px[0] as usize] += 1;
        h[1][px[1] as usize] += 1;
        h[2][px[2] as usize] += 1;
    }
    h
}

/// The share of a frame's samples one edge bin must hold for the panel to mark clipping.
///
/// ⚠ A marker that fires on the one specular every frame has is a marker nobody reads; one
/// that never fires on a blown sky is worse. 0.5% of the frame is the line.
pub const CLIP_FRACTION: f64 = 0.005;

/// Whether the shadows (bin 0) and the highlights (bin 255) are clipped in any channel:
/// `(left, right)`.
pub fn clipped_edges(hist: &[[u32; 256]; 3]) -> (bool, bool) {
    let total: u64 = hist[0].iter().map(|&c| c as u64).sum();
    if total == 0 {
        return (false, false);
    }
    let limit = (total as f64 * CLIP_FRACTION) as u64;
    let left = hist.iter().any(|c| c[0] as u64 > limit);
    let right = hist.iter().any(|c| c[255] as u64 > limit);
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clipping_marker_needs_more_than_a_single_pixel() {
        let mut h = [[0u32; 256]; 3];
        h[0][128] = 1000;
        h[1][128] = 1000;
        h[2][128] = 1000;
        h[0][255] = 1; // one hot pixel in red
        assert_eq!(clipped_edges(&h), (false, false));
        h[2][0] = 10; // 1% of blue at black
        assert_eq!(clipped_edges(&h), (true, false));
        h[0][255] = 10;
        assert_eq!(clipped_edges(&h), (true, true));
        assert_eq!(
            clipped_edges(&[[0; 256]; 3]),
            (false, false),
            "an empty frame clips nothing"
        );
    }

    #[test]
    fn counts_each_channel() {
        let rgba = vec![0, 128, 255, 255, 0, 128, 255, 255];
        let h = histogram_rgba(&rgba);
        assert_eq!(h[0][0], 2);
        assert_eq!(h[1][128], 2);
        assert_eq!(h[2][255], 2);
        assert_eq!(h[0][255], 0);
    }

    #[test]
    fn empty_is_all_zero() {
        let h = histogram_rgba(&[]);
        assert!(h.iter().all(|c| c.iter().all(|&n| n == 0)));
    }
}
