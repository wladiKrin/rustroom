//! Histogram of a display (sRGB, 8-bit RGBA) frame.

/// Per-channel 256-bin counts [R, G, B] from an interleaved RGBA byte buffer.
pub fn histogram_rgba(rgba: &[u8]) -> [[u32; 256]; 3] {
    let mut h = [[0u32; 256]; 3];
    for px in rgba.chunks_exact(4) {
        h[0][px[0] as usize] += 1;
        h[1][px[1] as usize] += 1;
        h[2][px[2] as usize] += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

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
