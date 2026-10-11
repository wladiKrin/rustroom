//! A scanned film negative turned into a positive (M10.11a).
//!
//! ⚠⚠ **RADIOMETRIC, IN LINEAR LIGHT — NOT `1 − x`.** Film records density `d = γ · log10(E)`, so the
//! light a scan passes is `base · 10^−d`. Dividing the base out and undoing the film's contrast,
//! `10^(log10(base / px) / γ)`, gives back the scene's exposure: scene-linear light, which is what
//! every stage after this one expects. Flipping the encoded values instead (Lightroom's curve trick)
//! leaves the orange mask's colour in every tone and makes every later slider run backwards.
//!
//! ⚠ Then levels, per channel, from **measured densities** (`low` → black, `high` → [`TARGET`]): the
//! three dye layers of a colour negative differ in density range, and levelling each on its own is
//! what neutralises the mask's remaining casts — Negative Lab Pro's auto balance, in linear light.
//!
//! ⚠ A prefix step right after lens correction — see `build_prefix_steps`.

use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::Negative;

/// Where the measured highlight density lands: under white, so what is brighter than the frame's
/// 99.5th percentile — a specular, a lamp — keeps some headroom.
pub const TARGET: f32 = 0.9;

/// Below this the scan is treated as fully blocked, so a black pixel is a bright one rather than
/// an infinity.
const FLOOR: f32 = 1e-6;

/// Each channel's density above the film base — or, for a black and white negative, the density of
/// the mean of the three ratios in all three.
pub fn density(rgb: [f32; 3], n: &Negative) -> [f32; 3] {
    let ratio: [f32; 3] = std::array::from_fn(|c| rgb[c].max(FLOOR) / n.base[c]);
    if n.mono {
        let d = -((ratio[0] + ratio[1] + ratio[2]) / 3.0).log10();
        [d; 3]
    } else {
        ratio.map(|r| -r.log10())
    }
}

/// One pixel of the negative as the positive — `rgb` itself when `n` is not active, so a reader can
/// call this unconditionally.
pub fn positive(rgb: [f32; 3], n: &Negative) -> [f32; 3] {
    if !n.is_active() {
        return rgb;
    }
    let d = density(rgb, n);
    let (low, high) = if n.mono {
        let mean = |v: [f32; 3]| (v[0] + v[1] + v[2]) / 3.0;
        ([mean(n.low); 3], [mean(n.high); 3])
    } else {
        (n.low, n.high)
    };
    let light = |d: f32| 10f32.powf(d / n.gamma);
    std::array::from_fn(|c| {
        let (black, white) = (light(low[c]), light(high[c]));
        ((light(d[c]) - black) / (white - black)).max(0.0) * TARGET
    })
}

/// The negative stage: [`positive`] over every pixel.
pub struct NegativeStage {
    n: Negative,
}

impl NegativeStage {
    pub fn new(n: &Negative) -> Self {
        Self { n: *n }
    }
}

impl crate::per_pixel::PerPixel for NegativeStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        positive(p, &self.n)
    }
}

impl PipelineStage for NegativeStage {
    fn name(&self) -> &'static str {
        "negative"
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
    use edit_model::Negative;

    const BASE: [f32; 3] = [0.85, 0.55, 0.30];
    const GAMMA: f32 = 0.6;

    /// What film of contrast `GAMMA` and this orange base passes, for scene exposure `e` relative to
    /// the film's threshold (1 = just above the base).
    fn film(e: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| BASE[c] * 10f32.powf(-GAMMA * e[c].max(1.0).log10()))
    }

    fn negative(low: f32, high: f32) -> Negative {
        Negative {
            on: true,
            base: BASE,
            gamma: GAMMA,
            low: [low; 3],
            high: [high; 3],
            mono: false,
        }
    }

    #[test]
    fn a_film_negative_inverts_back_to_its_scene() {
        // ⚠ Scene exposures 1..1000 in three colours, through the film model: densities 0..1.8.
        let n = negative(0.0, 1.8);
        for e in [
            [1.0, 1.0, 1.0],
            [10.0, 5.0, 2.0],
            [100.0, 300.0, 30.0],
            [1000.0, 700.0, 1000.0],
            [2.5, 40.0, 600.0],
        ] {
            let out = positive(film(e), &n);
            for c in 0..3 {
                let want = (e[c] - 1.0) / 999.0 * TARGET;
                assert!(
                    (out[c] - want).abs() <= 1e-3 * want.max(1e-3) + 1e-5,
                    "{e:?} channel {c}: {} against {want}",
                    out[c]
                );
            }
        }
    }

    #[test]
    fn the_base_becomes_black_and_the_densest_point_the_target() {
        let n = negative(0.0, 1.8);
        assert_eq!(positive(BASE, &n), [0.0; 3]);
        let densest = std::array::from_fn(|c| BASE[c] * 10f32.powf(-1.8));
        for v in positive(densest, &n) {
            assert!((v - TARGET).abs() < 1e-4, "{v}");
        }
        // ⚠ A different contrast keeps both ends where the measured densities put them.
        let soft = Negative { gamma: 0.9, ..n };
        assert_eq!(positive(BASE, &soft), [0.0; 3]);
        for v in positive(densest, &soft) {
            assert!((v - TARGET).abs() < 1e-4, "{v}");
        }
        // …and moves the middle: a different contrast is a different positive.
        let mid = film([31.6; 3]);
        assert!((positive(mid, &n)[0] - positive(mid, &soft)[0]).abs() > 0.01);
    }

    #[test]
    fn each_channel_is_levelled_on_its_own_densities() {
        let n = Negative {
            low: [0.1, 0.2, 0.3],
            high: [1.5, 1.7, 1.9],
            ..negative(0.0, 1.8)
        };
        for c in 0..3 {
            let at = |d: f32| {
                let mut px = BASE;
                px[c] = BASE[c] * 10f32.powf(-d);
                positive(px, &n)[c]
            };
            assert!(at(n.low[c]).abs() < 1e-5, "channel {c} black");
            assert!((at(n.high[c]) - TARGET).abs() < 1e-4, "channel {c} white");
        }
    }

    #[test]
    fn black_and_white_gives_one_neutral_channel() {
        let n = Negative {
            mono: true,
            ..negative(0.0, 1.8)
        };
        let out = positive(film([50.0, 20.0, 400.0]), &n);
        assert!(out[0] > 0.0);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[1], out[2]);
    }

    #[test]
    fn brighter_than_the_base_is_black_and_nothing_is_nan() {
        let n = negative(0.0, 1.8);
        assert_eq!(positive([1.0, 0.9, 0.8], &n), [0.0; 3], "a sprocket hole");
        let blocked = positive([0.0, 0.0, 0.0], &n);
        assert!(
            blocked.iter().all(|v| v.is_finite() && *v > 0.0),
            "{blocked:?}"
        );
    }

    #[test]
    fn an_inactive_negative_changes_nothing() {
        let px = [0.3, 0.2, 0.1];
        assert_eq!(positive(px, &Negative::default()), px);
        let unmeasured = Negative {
            high: [0.0; 3],
            ..negative(0.0, 1.8)
        };
        assert_eq!(positive(px, &unmeasured), px);
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, px);
        img.set_pixel(1, 0, BASE);
        let out = NegativeStage::new(&negative(0.0, 1.8)).apply(img);
        assert_eq!(out.pixel(1, 0), [0.0; 3], "the stage applies `positive`");
    }
}
