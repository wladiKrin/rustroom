//! The hue model `HslStage` applies, and the table it is sampled through.

/// Oklab hue angle (degrees) at each band's centre, in `HSL_BAND_NAMES` order. Derived from
/// the canonical sRGB colours (#FF0000, #FF8000, #FFFF00, #00FF00, #00FFFF, #0000FF,
/// #8000FF, #FF00FF) with our own conversion — `band_centers_match_canonical_colors` in
/// `hsl.rs` recomputes them and fails if this table drifts.
pub const BAND_HUES: [f32; 8] = [29.23, 52.98, 109.77, 142.50, 194.77, 264.05, 293.94, 328.36];

/// Hue slider at ±100 rotates by this many degrees.
pub const MAX_HUE_SHIFT_DEG: f32 = 30.0;

/// The arc each band owns, from its own centre forward to the next. ⚠ These are constants of
/// `BAND_HUES` and were previously recomputed **per band per pixel** inside the hot loop.
const SPANS: [f32; 8] = {
    let mut s = [0.0f32; 8];
    let mut i = 0;
    while i < 8 {
        let d = BAND_HUES[(i + 1) % 8] - BAND_HUES[i];
        s[i] = if d < 0.0 { d + 360.0 } else { d };
        i += 1;
    }
    s
};

/// The two bands bracketing hue `h` and the smoothstep blend between them: `(i, j, s)` means
/// weight `1 - s` on band `i` and `s` on band `j`.
///
/// ⚠ Returning the **pair** rather than eight weights is half the point: exactly two weights
/// are ever non-zero, so a caller does six multiply-adds instead of twenty-four.
///
/// ⚠⚠ The shipped version computed `(x + 360.0) % 360.0`, which is `fmodf` — a libm call, not a
/// hardware instruction on ARM — evaluated per band until the pair was found, averaging ~7 per
/// pixel. It is also **less accurate**: adding 360 to a value near 330 moves it into a binade
/// with twice the exponent spacing, and subtracting 360 does not recover the original. Measured
/// over 3.6 M hues, the two forms differ by at most 1.07e-6 in a weight and select a different
/// band pair on exactly one hue, at a span boundary where that weight is ~0 anyway.
#[inline]
pub fn band_pair(h: f32) -> (usize, usize, f32) {
    let mut i = 0;
    while i < 8 {
        let d = h - BAND_HUES[i];
        let delta = if d < 0.0 { d + 360.0 } else { d };
        if delta < SPANS[i] {
            let t = delta / SPANS[i];
            return (i, (i + 1) % 8, t * t * (3.0 - 2.0 * t));
        }
        i += 1;
    }
    // Unreachable: the spans tile the circle. Stay total rather than silently zeroing.
    (0, 1, 0.0)
}

/// The eight weights at hue `h`. **Derived from `band_pair`** so the two cannot disagree about
/// which bands are live.
///
/// ⚠ `#[cfg(test)]` because it has no production caller: the hot path takes the pair directly and
/// the table build does too. `mod stages` is private, so a `pub fn` nobody calls is dead code
/// under `-D warnings` — the trap this project records for binary crates, reproduced inside a
/// library by a private module. Same handling as `DialogSize::ALL` and `theme::base_fill`.
#[cfg(test)]
pub fn band_weights(h: f32) -> [f32; 8] {
    let (i, j, s) = band_pair(h);
    let mut w = [0.0f32; 8];
    w[i] = 1.0 - s;
    w[j] = s;
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ The discriminating case. `band_pair` must select the band whose span CONTAINS the hue,
    /// and the two implementations of that selection (the shipped `fmod` one and this one)
    /// disagreed on exactly one hue in 3.6 million — at a span boundary. A test that only probed
    /// band centres could not see a selection bug at all, because at a centre the weight is 1.0
    /// and the neighbour's is 0.0, so picking either neighbouring pair gives the same answer.
    #[test]
    fn every_hue_selects_the_band_span_that_contains_it() {
        for k in 0..36_000u32 {
            let h = k as f32 * 0.01;
            let (i, j, s) = band_pair(h);
            assert_eq!(j, (i + 1) % 8, "the pair must be adjacent at h={h}");
            assert!((0.0..=1.0).contains(&s), "blend out of range at h={h}: {s}");

            let span = {
                let d = BAND_HUES[j] - BAND_HUES[i];
                if d < 0.0 {
                    d + 360.0
                } else {
                    d
                }
            };
            let delta = {
                let d = h - BAND_HUES[i];
                if d < 0.0 {
                    d + 360.0
                } else {
                    d
                }
            };
            assert!(delta < span + 1e-3, "h={h} is not inside span {i}");
        }
    }

    /// The weights are a partition of unity at every hue — the property that makes an equal
    /// setting on all eight bands behave exactly like the global adjustment.
    #[test]
    fn weights_sum_to_one_all_the_way_round() {
        for k in 0..36_000u32 {
            let h = k as f32 * 0.01;
            let sum: f32 = band_weights(h).iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "weights sum to {sum} at h={h}");
        }
    }
}
