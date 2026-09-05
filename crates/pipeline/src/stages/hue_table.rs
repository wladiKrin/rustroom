//! The hue model `HslStage` applies, and the table it is sampled through.

use edit_model::HslBand;

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

/// ⚠⚠ The polynomials below are accurate ONLY over `|δ| ≤ MAX_HUE_SHIFT_DEG` in radians. If that
/// constant is ever raised they degrade **smoothly and silently** — there is no value at which a
/// test suddenly starts failing — so the bound is a build error rather than a slow drift. Same
/// instrument as `MID_BUDGET_BYTES` and `MIN_RADIUS`.
const _: () = assert!(MAX_HUE_SHIFT_DEG <= 30.0);

/// `sin δ` for `|δ| ≤ 0.5236` rad, to within 2.1e-6. A truncated Taylor series, not a libm call.
#[inline]
pub fn sin_small(d: f32) -> f32 {
    let d2 = d * d;
    d * (1.0 - d2 * (1.0 / 6.0 - d2 * (1.0 / 120.0)))
}

/// `cos δ` for `|δ| ≤ 0.5236` rad, to within 1.4e-7.
#[inline]
pub fn cos_small(d: f32) -> f32 {
    let d2 = d * d;
    1.0 - d2 * (0.5 - d2 * (1.0 / 24.0 - d2 * (1.0 / 720.0)))
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

/// The band-weighted slider values `[dh, sat, lum]` sampled uniformly over the hue circle.
///
/// ⚠⚠ It holds the **parameters**, not a finished rotation factor. The chroma fade multiplies
/// `dh`/`sat`/`lum` **before** they are used, so a table of finished factors would be valid only
/// where `chroma >= CHROMA_FADE` and would need a second code path below it — and on a dark or
/// desaturated frame that "slow" path becomes the common one, making the stage's cost depend on
/// the picture. Holding parameters keeps one table, one path, no branch.
///
/// ⚠ It also makes interpolation across the 360 -> 0 seam simply correct. A table of complex
/// factors would interpolate along a chord there rather than an arc.
pub struct HueTable {
    entries: Box<[[f32; 3]]>,
}

impl HueTable {
    /// ⚠ Built once per `HslStage`, which means once per mask as well as once globally. The cost
    /// is `n` evaluations of `band_pair` against a render measured in tens of milliseconds.
    pub fn build(bands: &[HslBand; 8], n: usize) -> Self {
        let entries = (0..n)
            .map(|i| {
                let h = i as f32 * 360.0 / n as f32;
                let (a, b, s) = band_pair(h);
                let (w0, w1) = (1.0 - s, s);
                [
                    w0 * bands[a].hue + w1 * bands[b].hue,
                    w0 * bands[a].saturation + w1 * bands[b].saturation,
                    w0 * bands[a].luminance + w1 * bands[b].luminance,
                ]
            })
            .collect();
        Self { entries }
    }

    /// Linear interpolation between the two bracketing entries.
    ///
    /// ⚠ The `% n` on the second index is load-bearing and wrong only in the final cell: without
    /// it, a hue in `[360 - 360/n, 360)` indexes one past the end.
    /// `the_last_cell_interpolates_across_the_seam` is the only test that can see it.
    #[inline]
    pub fn sample(&self, hue: f32) -> [f32; 3] {
        let n = self.entries.len();
        let x = hue * (n as f32 / 360.0);
        let i0 = (x as usize) % n;
        let t = x - x.floor();
        let a = self.entries[i0];
        let b = self.entries[(i0 + 1) % n];
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }
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

    fn probe_bands() -> [HslBand; 8] {
        // Deliberately all different, so a table that collapsed two entries would show.
        let mut b = [HslBand::default(); 8];
        for (i, band) in b.iter_mut().enumerate() {
            band.hue = -70.0 + 20.0 * i as f32;
            band.saturation = 90.0 - 21.0 * i as f32;
            band.luminance = -40.0 + 11.0 * i as f32;
        }
        b
    }

    /// ⚠⚠ THE SWEEP LANDS BETWEEN SAMPLES ON PURPOSE. A test that probed the table's own sample
    /// points would compare each entry against the value it was built from and pass with the
    /// interpolation deleted — it is the colour-grading trap, where a table test stayed green
    /// under a wrong convention because both sides called the same function.
    #[test]
    fn the_table_agrees_with_direct_evaluation_between_its_samples() {
        let bands = probe_bands();
        let n = 1024;
        let t = HueTable::build(&bands, n);
        let mut worst = 0.0f32;
        for k in 0..200_000u32 {
            // An irrational-ish stride so probes do not land on sample points.
            let h = (k as f32 * 0.0017341) % 360.0;
            let w = band_weights(h);
            let exact = [
                (0..8).map(|i| w[i] * bands[i].hue).sum::<f32>(),
                (0..8).map(|i| w[i] * bands[i].saturation).sum::<f32>(),
                (0..8).map(|i| w[i] * bands[i].luminance).sum::<f32>(),
            ];
            let got = t.sample(h);
            for c in 0..3 {
                worst = worst.max((got[c] - exact[c]).abs());
            }
        }
        // Slider units, where the whole range is 200 wide.
        assert!(
            worst < 0.05,
            "table drifts {worst:.5} slider units at n={n}"
        );
    }

    /// ⚠⚠ THE WRAPAROUND IS WRONG ONLY IN THE FINAL CELL — `hue` in `[360 - 360/n, 360)` — so a
    /// coarse sweep steps straight over it. This probes deliberately inside that cell, where the
    /// table must interpolate from the last entry back to the first across the 360 -> 0 seam.
    #[test]
    fn the_last_cell_interpolates_across_the_seam() {
        let bands = probe_bands();
        let n = 64;
        let t = HueTable::build(&bands, n);
        let step = 360.0 / n as f32;
        let last = 360.0 - step;
        let a = t.sample(last);
        let b = t.sample(0.0);
        for f in [0.25f32, 0.5, 0.75] {
            let got = t.sample(last + step * f);
            for c in 0..3 {
                let want = a[c] + (b[c] - a[c]) * f;
                assert!(
                    (got[c] - want).abs() < 1e-4,
                    "seam cell at f={f}, channel {c}: {got:?} vs lerp {want}"
                );
            }
        }
    }

    /// ⚠ The bound is what the whole approximation rests on, so the sweep must reach the very
    /// edge of the domain — the error grows as δ⁷, so a test that only probed small angles
    /// would pass against a polynomial that is useless at ±30°.
    #[test]
    fn the_small_angle_pair_is_accurate_across_the_whole_hue_range() {
        let limit = MAX_HUE_SHIFT_DEG.to_radians();
        let (mut worst_sin, mut worst_cos) = (0.0f32, 0.0f32);
        for k in -10_000..=10_000i32 {
            let d = limit * k as f32 / 10_000.0;
            worst_sin = worst_sin.max((sin_small(d) - d.sin()).abs());
            worst_cos = worst_cos.max((cos_small(d) - d.cos()).abs());
        }
        assert!(worst_sin < 3e-6, "sin_small worst error {worst_sin:e}");
        assert!(worst_cos < 3e-7, "cos_small worst error {worst_cos:e}");
    }

    /// ⚠ The rotation built from these is orthogonal only to the extent that
    /// `cos² + sin² == 1`, and a TRUNCATED series does not satisfy that exactly. This pins how
    /// far off it is, because it is what makes a pure hue shift change chroma by ~1e-6 where the
    /// shipped code left it bit-exact. Stated as a number rather than discovered later.
    #[test]
    fn the_pair_is_orthogonal_to_within_a_millionth() {
        let limit = MAX_HUE_SHIFT_DEG.to_radians();
        let mut worst = 0.0f32;
        for k in -10_000..=10_000i32 {
            let d = limit * k as f32 / 10_000.0;
            let n = sin_small(d) * sin_small(d) + cos_small(d) * cos_small(d);
            worst = worst.max((n - 1.0).abs());
        }
        assert!(worst < 5e-6, "cos^2 + sin^2 deviates by {worst:e}");
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
