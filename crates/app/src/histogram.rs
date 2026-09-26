//! Histogram of a display (sRGB, 8-bit RGBA) frame.

/// Per-channel 256-bin counts [R, G, B] from an interleaved RGBA byte buffer.
///
/// ⚠⚠ **Parallel since 2026-09-26, and EXACT rather than approximate.** It is built on every
/// render, and one thread over a 6.8 Mpx drag frame cost **12.4 ms of an 84.5 ms Straighten
/// frame**, the third-largest item in it. Each chunk counts into its own table and the tables are
/// summed; integer addition does not care how the frame was split, so the result is bit-identical
/// to the one-thread count, which `the_parallel_histogram_equals_the_serial_one` pins.
pub fn histogram_rgba(rgba: &[u8]) -> [[u32; 256]; 3] {
    use rayon::prelude::*;
    // ⚠ Big enough that the per-chunk 3 KB table and the final sum are noise; small enough that
    // a 1.7 Mpx fit frame still splits across every core.
    const CHUNK_PX: usize = 1 << 16;
    let count = |mut h: [[u32; 256]; 3], chunk: &[u8]| {
        for px in chunk.as_chunks::<4>().0 {
            h[0][px[0] as usize] += 1;
            h[1][px[1] as usize] += 1;
            h[2][px[2] as usize] += 1;
        }
        h
    };
    rgba.par_chunks(CHUNK_PX * 4)
        .fold(|| [[0u32; 256]; 3], count)
        .reduce(
            || [[0u32; 256]; 3],
            |mut a, b| {
                for (ac, bc) in a.iter_mut().zip(&b) {
                    for (x, y) in ac.iter_mut().zip(bc) {
                        *x += y;
                    }
                }
                a
            },
        )
}

/// The bins left out of the scale: pure black and pure white.
///
/// ⚠⚠ A blown sky or a crushed black is an **edge**, not tonal content, and it is the commonest cause
/// of a spike that flattens the graph. They are still drawn — clamped by the mesh — and
/// [`clipped_edges`] already reports them, so nothing is hidden by leaving them out of the scale.
///
/// ⚠ Exactly the two bins the clipping markers describe. Widening this to "the outer few bins" would
/// make the marker and the scale disagree about what an edge is.
const EDGE_BINS: [usize; 2] = [0, 255];

/// The tallest bin that counts towards the scale, across every channel, square-rooted.
///
/// ⚠ Falls back to the overall maximum when every non-edge bin is empty: a frame whose only content
/// sits at the two edges would otherwise divide by zero and paint `NaN`.
fn scale_reference(hist: &[[u32; 256]; 3]) -> f32 {
    let tallest = |skip_edges: bool| {
        hist.iter()
            .flat_map(|c| c.iter().enumerate())
            .filter(move |(i, _)| !skip_edges || !EDGE_BINS.contains(i))
            .map(|(_, &n)| n)
            .max()
            .unwrap_or(0)
    };
    let m = match tallest(true) {
        0 => tallest(false),
        n => n,
    };
    (m as f32).sqrt()
}

/// Normalised bar heights per channel, in `0.0..=1.0`.
///
/// ⚠⚠ **Square root, not linear** (user's choice of four, 2026-09-23). Linear scaling to the tallest
/// bin squashed ordinary tonal content onto the floor whenever the frame held one large flat area —
/// which read both as "large peaks drown out the whole view" and as "very static", because a shape
/// pinned at zero does not visibly move when you edit.
///
/// ⚠ Chosen over log, which flatters almost-empty bins so that sensor noise in an empty shadow reads
/// as tonal content. ⚠ The cost, accepted: heights are no longer proportional to quantity — twice as
/// tall is four times as many samples. Every histogram that compresses gives that up.
pub fn normalised(hist: &[[u32; 256]; 3]) -> [[f32; 256]; 3] {
    let scale = scale_reference(hist);
    if scale <= 0.0 {
        return [[0.0; 256]; 3];
    }
    std::array::from_fn(|c| std::array::from_fn(|i| ((hist[c][i] as f32).sqrt() / scale).min(1.0)))
}

/// One curve for the tone curve's ghost: the per-bin maximum across the channels, on the **same**
/// scale as [`normalised`].
///
/// ⚠ The maximum rather than the sum or the mean, so a saturated colour is not hidden by its dark
/// siblings — the tone curve already worked this way and this keeps it.
pub fn envelope(hist: &[[u32; 256]; 3]) -> [f32; 256] {
    let n = normalised(hist);
    std::array::from_fn(|i| n.iter().map(|c| c[i]).fold(0.0f32, f32::max))
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

    /// The one-thread count, kept as the reference the parallel version must equal exactly.
    fn serial(rgba: &[u8]) -> [[u32; 256]; 3] {
        let mut h = [[0u32; 256]; 3];
        for px in rgba.as_chunks::<4>().0 {
            h[0][px[0] as usize] += 1;
            h[1][px[1] as usize] += 1;
            h[2][px[2] as usize] += 1;
        }
        h
    }

    /// ⚠⚠ **Equal, not close**: counting is integer addition, so however the frame is split
    /// across threads the sums are the same. A frame large enough to be split into many chunks,
    /// with a length that is NOT a multiple of the chunk size, so the ragged last chunk is counted.
    #[test]
    fn the_parallel_histogram_equals_the_serial_one() {
        let n = 1_000_003usize;
        let mut state: u32 = 0x1234_5678;
        let mut rgba = Vec::with_capacity(n * 4);
        for _ in 0..n {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            rgba.extend_from_slice(&state.to_le_bytes());
        }
        let want = serial(&rgba);
        // ⚠ The fixture must fill every bin, or a dropped chunk could hide in an empty one.
        assert!(want.iter().all(|c| c.iter().all(|&v| v > 0)));
        assert_eq!(histogram_rgba(&rgba), want);
        assert_eq!(histogram_rgba(&[]), [[0u32; 256]; 3]);
    }

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

    /// The well is 72 pt tall (`HIST_HEIGHT`), so this converts a normalised height into the pixels
    /// actually painted — the only units in which "legible" means anything.
    fn pixels(v: f32) -> f32 {
        v * 72.0
    }

    /// ⚠⚠ **The report, as a test (2026-09-23).** A large flat area — a sky, a wall — puts one
    /// enormous count in one bin. Scaling linearly to the tallest bin squashed everything else onto
    /// the floor, which is both "large peaks drown out the whole view" *and* "very static": if the
    /// shape is pinned at zero, an edit that moves midtones moves nothing you can see.
    ///
    /// ⚠ Measured at the ratios a photograph actually produces. Linear against square root, in pixels
    /// of the 72 pt well:
    ///
    /// | peak : detail | linear | sqrt |
    /// |---|---|---|
    /// | 10:1 | 7.2 px | 22.8 px |
    /// | 100:1 | **0.7 px** | 7.2 px |
    /// | 200:1 | **0.4 px** | 5.1 px |
    #[test]
    fn a_large_peak_no_longer_flattens_ordinary_tonal_content() {
        for ratio in [10u32, 50, 100, 200] {
            let mut h = [[0u32; 256]; 3];
            h[0][128] = 1_000_000;
            h[0][64] = 1_000_000 / ratio;
            let n = normalised(&h);
            assert!(
                pixels(n[0][64]) >= 5.0,
                "at {ratio}:1 the detail drew {:.2} px, which is not legible",
                pixels(n[0][64])
            );
            assert!(
                n[0][128] > n[0][64],
                "and the peak must still read as the tallest"
            );
        }
    }

    /// ⚠⚠ **The honest limit, pinned rather than hidden.** Square root compresses a ratio by its own
    /// square root, so it does not make an *arbitrarily* large peak harmless: at 2000:1 — a bin holding
    /// 4% of the frame beside one holding 0.002% — the small bar is **1.6 px**, up from 0.04 px but
    /// still a sliver.
    ///
    /// ⚠ This is the shape of the trade the user chose over a percentile clip, which would have kept
    /// linear proportions and cut the peak off instead. **If real photographs still drown, the next
    /// lever is log or a minimum-height floor, not a bigger multiplier here.** Do not "fix" this test
    /// by lowering its threshold.
    #[test]
    fn square_root_does_not_rescue_an_extreme_ratio_and_that_is_known() {
        let mut h = [[0u32; 256]; 3];
        h[0][128] = 1_000_000;
        h[0][64] = 500; // 2000:1
        let n = normalised(&h);
        let px = pixels(n[0][64]);
        assert!(
            (1.0..3.0).contains(&px),
            "expected a ~1.6 px sliver at 2000:1, got {px:.2} px"
        );
        assert!(
            px > pixels(500.0 / 1_000_000.0) * 40.0,
            "it is still a 40x improvement on linear"
        );
    }

    /// ⚠⚠ The targeted half of the fix: a blown sky or a crushed black is an **edge**, not tonal
    /// content, and it must not set the scale for the whole graph.
    #[test]
    fn a_clipped_edge_does_not_set_the_scale() {
        let mut h = [[0u32; 256]; 3];
        h[0][255] = 1_000_000; // blown highlights
        h[0][100] = 400;
        let n = normalised(&h);
        assert!(
            n[0][100] > 0.9,
            "the midtone should nearly fill the well, got {}",
            n[0][100]
        );
        let mut h2 = [[0u32; 256]; 3];
        h2[1][0] = 1_000_000; // crushed shadows
        h2[1][100] = 400;
        assert!(normalised(&h2)[1][100] > 0.9);
    }

    /// ⚠ Left out of the *scale*, still drawn — clamped rather than running off the top.
    #[test]
    fn the_edge_bins_are_still_drawn_and_clamped() {
        let mut h = [[0u32; 256]; 3];
        h[0][255] = 1_000_000;
        h[0][100] = 400;
        let n = normalised(&h);
        assert_eq!(n[0][255], 1.0, "drawn at full height, not above it");
        assert!(n[0][255].is_finite());
    }

    /// ⚠⚠ **The degenerate case, which would otherwise be `NaN`.** A frame whose only content sits at
    /// the two edges has a non-edge maximum of zero, and dividing by it paints nothing at all.
    #[test]
    fn a_frame_with_content_only_at_the_edges_is_finite() {
        let mut h = [[0u32; 256]; 3];
        h[0][0] = 5000;
        h[0][255] = 5000;
        let n = normalised(&h);
        assert!(
            n.iter().flat_map(|c| c.iter()).all(|v| v.is_finite()),
            "every height must be finite"
        );
        assert!(n[0][0] > 0.0, "and the content must be visible");
    }

    #[test]
    fn an_empty_frame_is_all_zero_and_does_not_divide_by_zero() {
        let n = normalised(&[[0u32; 256]; 3]);
        assert!(n.iter().flat_map(|c| c.iter()).all(|&v| v == 0.0));
    }

    /// ⚠ More samples is never drawn shorter. Obvious, and the property a bad scaling breaks.
    #[test]
    fn more_samples_is_never_drawn_shorter() {
        let mut h = [[0u32; 256]; 3];
        h[0][50] = 10;
        h[0][51] = 100;
        h[0][52] = 1000;
        let n = normalised(&h);
        assert!(n[0][50] < n[0][51] && n[0][51] < n[0][52]);
    }

    /// ⚠ The ghost is the per-bin maximum over the channels, so a saturated colour is not hidden by
    /// its dark siblings — the behaviour the tone curve already documented.
    #[test]
    fn the_envelope_takes_the_brightest_channel_per_bin() {
        let mut h = [[0u32; 256]; 3];
        h[0][100] = 1000; // red only
        h[1][100] = 1;
        h[2][100] = 1;
        h[1][150] = 1000; // green only
        let e = envelope(&h);
        assert!(e[100] > 0.9 && e[150] > 0.9);
        assert_eq!(e[200], 0.0);
    }

    /// ⚠ One scaling, two callers: the panel's histogram and the curve's ghost cannot drift apart.
    /// They each computed their own maximum before this.
    #[test]
    fn the_envelope_and_the_channels_share_one_scale() {
        let mut h = [[0u32; 256]; 3];
        h[0][100] = 1000;
        h[1][100] = 10;
        h[2][100] = 10;
        assert_eq!(envelope(&h)[100], normalised(&h)[0][100]);
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
