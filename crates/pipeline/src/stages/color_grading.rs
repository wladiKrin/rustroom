//! Three-way colour grading: a tint per tonal range, added in Oklab.

use crate::PipelineStage;
// ⚠ The **ProPhoto** pair, not the sRGB one: this stage converts the working space in and out
// again, and mixing the two directions produces a plausible-looking colour shift that is very
// hard to attribute later.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::{ColorGrading, GradeWheel};

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

/// Oklab chroma offset at Saturation 100. For scale, a vivid sRGB red has a chroma near 0.26.
const SAT_MAX: f32 = 0.15;
/// Oklab L offset at Luminance ±100.
const LUM_MAX: f32 = 0.15;
/// Entries in the lightness table. The weight curves are smooth, so 256 with linear
/// interpolation is well inside the tolerance the table is checked against.
const LUT_SIZE: usize = 256;

/// One wheel's contribution as an Oklab offset `(da, db, dl)`.
pub(crate) fn wheel_offset(w: &GradeWheel) -> [f32; 3] {
    let h = w.hue.to_radians();
    let s = (w.saturation / 100.0) * SAT_MAX;
    [s * h.cos(), s * h.sin(), (w.luminance / 100.0) * LUM_MAX]
}

/// A tint per tonal range, added in Oklab.
///
/// ⚠ **One stage, not three.** The tint at a pixel depends on nothing but its lightness and on
/// constants, so the whole three-wheel grade collapses to a 1-D table built once per render.
/// Only one stage can do that; three sequential stages would mean three Oklab round trips with
/// each range re-measuring a lightness the previous one had already moved — the interaction
/// `LocalContrastStage` sums deltas against one source specifically to avoid.
pub struct ColorGradingStage {
    /// `(da, db, dl)` per lightness, indexed by `l * (LUT_SIZE - 1)` and interpolated.
    lut: Vec<[f32; 3]>,
}

impl ColorGradingStage {
    /// The shadow / midtone / highlight weights at lightness `l`.
    ///
    /// ⚠⚠ **Exposed so the panel's range strip cannot re-derive them.** A strip that computed
    /// its own Gaussians would come to *lie* about where the ranges are — which is the exact
    /// failure the strip exists to prevent, since its whole job is showing what Blending and
    /// Balance did. Reached through the stage rather than through the free function because
    /// `pipeline`'s `stages` module is private and `ColorGradingStage` is already re-exported.
    pub fn range_weights(l: f32, blending: f32, balance: f32) -> [f32; 3] {
        range_weights(l, blending, balance)
    }

    pub fn new(g: &ColorGrading) -> Self {
        let wheels = [
            wheel_offset(&g.shadows),
            wheel_offset(&g.midtones),
            wheel_offset(&g.highlights),
        ];
        let global = wheel_offset(&g.global);

        let lut = (0..LUT_SIZE)
            .map(|i| {
                let l = i as f32 / (LUT_SIZE - 1) as f32;
                let w = range_weights(l, g.blending, g.balance);
                let mut o = global;
                for k in 0..3 {
                    o[0] += w[k] * wheels[k][0];
                    o[1] += w[k] * wheels[k][1];
                    o[2] += w[k] * wheels[k][2];
                }
                o
            })
            .collect();

        Self { lut }
    }

    /// The grade at lightness `l`, linearly interpolated between table entries.
    ///
    /// ⚠ The **index** clamps to `[0, 1]`; the caller's `l` does not. Scene-referred values
    /// above white have `L > 1`, and clamping the value rather than the index would drag every
    /// highlight back to white — destroying exactly the headroom the wide working space exists
    /// to preserve. Identical trap to `ProfileHueSatMap`'s `v`.
    fn grade_at(&self, l: f32) -> [f32; 3] {
        let t = l.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
        let i = (t.floor() as usize).min(LUT_SIZE - 1);
        let j = (i + 1).min(LUT_SIZE - 1);
        let f = t - i as f32;
        let (a, b) = (self.lut[i], self.lut[j]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    }

    /// The `(da, db, dl)` table, one entry per lightness step.
    ///
    /// ⚠⚠ **Exposed so a GPU twin uploads THIS table rather than rebuilding it.** Two
    /// constructions of one table is how they come to disagree about Blending and Balance — and
    /// nothing in the rendered image would announce it, because both answers are plausible
    /// photographs. The rule `HueTable::entries` already follows, for the same reason.
    ///
    /// ⚠ A caller reproducing the lookup must clamp the **index**, never the value — see the
    /// headroom warning on [`Self::grade_at`].
    pub fn lut(&self) -> &[[f32; 3]] {
        &self.lut
    }
}

impl crate::per_pixel::PerPixel for ColorGradingStage {
    fn px(&self, _x: usize, _y: usize, rgb: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let [da, db, dl] = self.grade_at(l);
        let out = oklab_to_prophoto([l + dl, a + da, b + db]);
        // Clamped at 0 only, matching `SaturationStage` and `HslStage`. Never at 1: the
        // output transform is the only place clipping belongs.
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    }
}

impl PipelineStage for ColorGradingStage {
    fn name(&self) -> &'static str {
        "color_grading"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
    }
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

    use core_image::ImageBuf;
    use edit_model::{ColorGrading, GradeWheel};

    fn apply_one(g: &ColorGrading, px: [f32; 3]) -> [f32; 3] {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, px);
        ColorGradingStage::new(g).apply(img).pixel(0, 0)
    }

    /// The grade at `l`, computed straight from the definition with no table involved —
    /// the reference the LUT is checked against.
    fn direct_grade(g: &ColorGrading, l: f32) -> [f32; 3] {
        let w = range_weights(l.clamp(0.0, 1.0), g.blending, g.balance);
        let wheels = [g.shadows, g.midtones, g.highlights];
        let mut o = wheel_offset(&g.global);
        for k in 0..3 {
            let c = wheel_offset(&wheels[k]);
            o[0] += w[k] * c[0];
            o[1] += w[k] * c[1];
            o[2] += w[k] * c[2];
        }
        o
    }

    fn split_tone() -> ColorGrading {
        ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 60.0,
                luminance: -20.0,
            },
            highlights: GradeWheel {
                hue: 45.0,
                saturation: 60.0,
                luminance: 15.0,
            },
            ..ColorGrading::default()
        }
    }

    #[test]
    fn a_neutral_grade_returns_the_pixel_unchanged() {
        // ⚠ Within rounding, not bit-exact: a neutral grade still round-trips through Oklab.
        // The stage is never *inserted* at neutral — `build_pipeline` skips it — which is
        // what keeps the existing goldens byte-identical.
        let px = [0.4, 0.25, 0.15];
        let out = apply_one(&ColorGrading::default(), px);
        for k in 0..3 {
            assert!((out[k] - px[k]).abs() < 1e-5, "{out:?} vs {px:?}");
        }
    }

    #[test]
    fn the_table_agrees_with_direct_evaluation() {
        let g = split_tone();
        let stage = ColorGradingStage::new(&g);
        for i in 0..=200 {
            let l = i as f32 / 200.0;
            let (got, want) = (stage.grade_at(l), direct_grade(&g, l));
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-3,
                    "l={l} component {k}: table {got:?} vs direct {want:?}"
                );
            }
        }
    }

    #[test]
    fn the_exposed_weights_are_the_ones_the_stage_actually_grades_with() {
        // ⚠ Derived from the stage's *rendered* behaviour, not compared against the free
        // function it delegates to — that comparison would be true by construction and blind
        // to the exposed copy being reimplemented. With only the shadow wheel tinted,
        // `grade_at(l)` is exactly `w[0] * wheel_offset(shadows)`, so the weight can be
        // recovered by division and checked against what the panel is handed.
        let shadows = GradeWheel {
            hue: 0.0,
            saturation: 80.0,
            luminance: 0.0,
        };
        for &(blending, balance) in &[(0.0, 0.0), (50.0, 0.0), (50.0, 60.0), (100.0, -40.0)] {
            let g = ColorGrading {
                shadows,
                blending,
                balance,
                ..ColorGrading::default()
            };
            let stage = ColorGradingStage::new(&g);
            let unit = wheel_offset(&shadows)[0];
            assert!(unit.abs() > 1e-3, "fixture check: the shadow tint is inert");
            for i in 0..=20 {
                let l = i as f32 / 20.0;
                let from_render = stage.grade_at(l)[0] / unit;
                let exposed = ColorGradingStage::range_weights(l, blending, balance)[0];
                assert!(
                    (from_render - exposed).abs() < 2e-3,
                    "l={l} blending={blending} balance={balance}: the stage grades with \
                     {from_render} but hands the panel {exposed}"
                );
            }
        }
    }

    #[test]
    fn three_wheels_alike_reproduce_a_uniform_tint() {
        // The payoff of sum-to-one: the same tint at every lightness, with no seams.
        let w = GradeWheel {
            hue: 200.0,
            saturation: 70.0,
            luminance: 30.0,
        };
        let g = ColorGrading {
            shadows: w,
            midtones: w,
            highlights: w,
            ..ColorGrading::default()
        };
        let stage = ColorGradingStage::new(&g);
        let want = wheel_offset(&w);
        for i in 0..=100 {
            let l = i as f32 / 100.0;
            let got = stage.grade_at(l);
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-5,
                    "l={l} component {k}: {got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn the_global_wheel_adds_on_top_of_the_weighted_ranges() {
        let base = split_tone();
        let extra = GradeWheel {
            hue: 90.0,
            saturation: 40.0,
            luminance: -25.0,
        };
        let with_global = ColorGrading {
            global: extra,
            ..base
        };
        let (a, b) = (
            ColorGradingStage::new(&base),
            ColorGradingStage::new(&with_global),
        );
        let want = wheel_offset(&extra);
        for i in 0..=50 {
            let l = i as f32 / 50.0;
            let (x, y) = (a.grade_at(l), b.grade_at(l));
            for k in 0..3 {
                assert!(
                    ((y[k] - x[k]) - want[k]).abs() < 1e-5,
                    "l={l} component {k}: delta {} vs {}",
                    y[k] - x[k],
                    want[k]
                );
            }
        }
    }

    #[test]
    fn a_grey_pixel_is_tinted() {
        // Unlike HSL, grading is deliberately **not** chroma-gated: it *assigns* a hue rather
        // than reading one, so there is no noisy hue to fade out — and tinting neutrals is
        // the entire point of shadow toning.
        let g = ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 80.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        let out = apply_one(&g, [0.05, 0.05, 0.05]);
        let [_, a, b] = color::oklab::prophoto_to_oklab(out);
        assert!(a.hypot(b) > 0.01, "grey stayed neutral: {out:?}");
    }

    #[test]
    fn the_tint_lands_at_the_hue_the_wheel_names() {
        // Pins the convention: a wheel's `hue` **is** the Oklab hue angle of the tint it
        // applies — the same angle `BAND_HUES` uses in the HSL stage.
        //
        // ⚠ This cannot be left to `the_table_agrees_with_direct_evaluation`, which calls
        // `wheel_offset` on *both* sides and so is blind to anything wrong inside it; nor to a
        // golden, which only ever catches a *change* and would happily bake a wrong convention
        // into its reference on the first run. This test derives the answer independently, by
        // measuring the chroma the stage actually produced.
        for &hue in &[0.0f32, 45.0, 90.0, 180.0, 240.0, 330.0] {
            let g = ColorGrading {
                global: GradeWheel {
                    hue,
                    saturation: 40.0,
                    luminance: 0.0,
                },
                ..ColorGrading::default()
            };
            // A neutral pixel carries no chroma of its own, so the output's chroma is the tint.
            let out = apply_one(&g, [0.4, 0.4, 0.4]);
            let [_, a, b] = color::oklab::prophoto_to_oklab(out);
            let got = b.atan2(a).to_degrees().rem_euclid(360.0);
            let err = (got - hue + 540.0).rem_euclid(360.0) - 180.0;
            assert!(err.abs() < 1.0, "hue {hue}: tint landed at {got}");
        }
    }

    #[test]
    fn a_pixel_above_white_keeps_its_headroom() {
        // ⚠ The **index** clamps to [0, 1]; the pixel's own L must not. Clamping the value
        // would drag every scene-referred highlight back to white, destroying exactly the
        // headroom the wide working space exists to preserve. Same trap as HueSatMap's `v`.
        let px = [2.0, 2.0, 2.0];
        let before = color::oklab::prophoto_to_oklab(px)[0];
        assert!(before > 1.2, "fixture is not above white: L={before}");

        let g = ColorGrading {
            highlights: GradeWheel {
                hue: 45.0,
                saturation: 60.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        let after = color::oklab::prophoto_to_oklab(apply_one(&g, px))[0];
        assert!(
            (after - before).abs() < 1e-4,
            "lightness moved: {before} -> {after}"
        );
    }

    #[test]
    fn the_output_is_clamped_at_zero_but_not_at_one() {
        // Matches `SaturationStage` and `HslStage`. Clipping at the top belongs in the output
        // transform and nowhere else.
        let g = ColorGrading {
            global: GradeWheel {
                hue: 30.0,
                saturation: 100.0,
                luminance: 100.0,
            },
            ..ColorGrading::default()
        };
        let out = apply_one(&g, [1.6, 1.6, 1.6]);
        assert!(out.iter().all(|&v| v >= 0.0), "negative sample in {out:?}");
        assert!(
            out.iter().any(|&v| v > 1.0),
            "headroom was clipped: {out:?}"
        );
    }
}
