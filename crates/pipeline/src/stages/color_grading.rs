//! Three-way colour grading: a tint per tonal range, added in Oklab.

/// Narrowest range width, at Blending 0 — near-hard selection.
const SIGMA_MIN: f32 = 0.08;
/// Widest, at Blending 100 — wide enough that all three weights sit within a few percent
/// of 1/3 across the whole lightness range, i.e. a uniform tint.
const SIGMA_MAX: f32 = 3.0;
/// How far Balance ±100 slides the three range centres, in Oklab L.
const BALANCE_SHIFT: f32 = 0.25;

/// Range centres in Oklab L before Balance slides them: black, mid, white.
///
/// ⚠ The ranges are selected on **Oklab L, not physical luminance**. We are already in Oklab
/// for the tint, so L is free — and mid-grey sits near 0.6 in L but at 0.18 in Y. Selecting
/// on Y would put "midtones" where most people would say highlights.
const CENTRES: [f32; 3] = [0.0, 0.5, 1.0];

/// The shadow / midtone / highlight weights at lightness `l`.
///
/// Three Gaussians, normalised — so they **sum to exactly 1 at every lightness**. That is what
/// makes three equal wheels reproduce that tint uniformly with no seams, and it is why the
/// Global wheel is a plain fourth constant rather than a fourth range.
pub(crate) fn range_weights(l: f32, blending: f32, balance: f32) -> [f32; 3] {
    let sigma = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN).powf(blending.clamp(0.0, 100.0) / 100.0);
    let shift = -(balance.clamp(-100.0, 100.0) / 100.0) * BALANCE_SHIFT;

    let mut e = [0.0f32; 3];
    for i in 0..3 {
        let d = (l - (CENTRES[i] + shift)) / sigma;
        e[i] = -(d * d);
    }

    // ⚠ Subtracting the maximum exponent before exponentiating is load-bearing, not tidiness.
    // Without it, an `l` far from every centre at small sigma underflows all three
    // exponentials to zero and the normalisation divides by zero. With it the largest raw
    // weight is exactly 1, so the sum is always >= 1.
    let m = e[0].max(e[1]).max(e[2]);
    let raw = [(e[0] - m).exp(), (e[1] - m).exp(), (e[2] - m).exp()];
    let sum = raw[0] + raw[1] + raw[2];
    [raw[0] / sum, raw[1] / sum, raw[2] / sum]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLENDINGS: [f32; 5] = [0.0, 25.0, 50.0, 75.0, 100.0];
    const BALANCES: [f32; 5] = [-100.0, -50.0, 0.0, 50.0, 100.0];

    #[test]
    fn the_weights_sum_to_one_at_every_lightness() {
        for &blending in &BLENDINGS {
            for &balance in &BALANCES {
                for i in 0..=100 {
                    let l = i as f32 / 100.0;
                    let w = range_weights(l, blending, balance);
                    let sum = w[0] + w[1] + w[2];
                    assert!(
                        (sum - 1.0).abs() < 1e-5,
                        "l={l} blending={blending} balance={balance} sum={sum}"
                    );
                    assert!(w.iter().all(|&x| x >= 0.0), "negative weight in {w:?}");
                }
            }
        }
    }

    #[test]
    fn blending_zero_selects_one_range_almost_exclusively() {
        assert!(range_weights(0.0, 0.0, 0.0)[0] > 0.99);
        assert!(range_weights(0.5, 0.0, 0.0)[1] > 0.99);
        assert!(range_weights(1.0, 0.0, 0.0)[2] > 0.99);
    }

    #[test]
    fn blending_one_hundred_weighs_the_ranges_almost_equally() {
        for i in 0..=10 {
            let l = i as f32 / 10.0;
            let w = range_weights(l, 100.0, 0.0);
            for &x in &w {
                assert!((x - 1.0 / 3.0).abs() < 0.03, "l={l} w={w:?}");
            }
        }
    }

    #[test]
    fn positive_balance_lets_the_highlight_tint_reach_lower() {
        let l = 0.4;
        let neutral = range_weights(l, 50.0, 0.0)[2];
        let shifted = range_weights(l, 50.0, 80.0)[2];
        assert!(
            shifted > neutral,
            "highlight weight at l={l}: balance 0 gave {neutral}, +80 gave {shifted}"
        );
    }

    #[test]
    fn an_extreme_lightness_at_the_narrowest_width_still_normalises() {
        // The case the max-subtraction exists for: every exponent hugely negative at once.
        for &balance in &BALANCES {
            for &l in &[-2.0f32, 0.0, 1.0, 3.0] {
                let w = range_weights(l, 0.0, balance);
                let sum = w[0] + w[1] + w[2];
                assert!(
                    (sum - 1.0).abs() < 1e-5 && sum.is_finite(),
                    "l={l} balance={balance} sum={sum} w={w:?}"
                );
            }
        }
    }
}
