use crate::PipelineStage;
// ⚠ The **ProPhoto** pair, not the sRGB one: this stage converts the working space in
// and out again, and mixing the two directions produces a smooth, plausible colour shift
// that is very hard to attribute later.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::{Hsl, HslBand};

/// Oklab hue angle (degrees) at each band's center, in `HSL_BAND_NAMES` order. Derived from
/// the canonical sRGB colors (#FF0000, #FF8000, #FFFF00, #00FF00, #00FFFF, #0000FF,
/// #8000FF, #FF00FF) with our own conversion — `band_centers_match_canonical_colors`
/// recomputes them and fails if this table drifts.
const BAND_HUES: [f32; 8] = [29.23, 52.98, 109.77, 142.50, 194.77, 264.05, 293.94, 328.36];

/// Below this chroma a pixel is neutral: it has no meaningful hue to band-match.
const EPS: f32 = 1e-6;
/// Adjustments fade in over this chroma range. A near-neutral pixel's hue is dominated by
/// sensor noise (measured chroma ~0.003 for ±0.6% noise), so neighbouring pixels would land
/// in different bands and get different treatment — visible speckle, worst on bright
/// low-saturation detail. Real colors sit far above this (a warm hair pixel is ~0.06), so
/// they keep full strength. The cost is that genuinely pastel colors respond less, which is
/// the honest behavior: you cannot meaningfully "select the orange" in something barely
/// orange.
const CHROMA_FADE: f32 = 0.02;
/// Hue slider at ±100 rotates by this many degrees.
const MAX_HUE_SHIFT_DEG: f32 = 30.0;

/// Weights for the 8 bands at hue `h` (degrees): a smoothstep blend between the two
/// bracketing band centers, zero elsewhere. The band spans tile the circle, so the weights
/// always sum to 1 — which is what makes an equal setting on every band behave exactly like
/// the corresponding global adjustment.
fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    let n = BAND_HUES.len();
    for i in 0..n {
        let next = (i + 1) % n;
        let span = (BAND_HUES[next] - BAND_HUES[i] + 360.0) % 360.0;
        let delta = (h - BAND_HUES[i] + 360.0) % 360.0;
        if delta < span {
            let t = delta / span;
            let s = t * t * (3.0 - 2.0 * t); // smoothstep
            w[i] = 1.0 - s;
            w[next] = s;
            return w;
        }
    }
    // Unreachable: the spans cover the full circle. Stay total rather than silently zeroing.
    w[0] = 1.0;
    w
}

/// Per-hue Hue/Saturation/Luminance, applied in Oklab so bands are perceptually even and a
/// lightness change doesn't drag hue with it.
pub struct HslStage {
    bands: [HslBand; 8],
}

impl HslStage {
    pub fn new(hsl: &Hsl) -> Self {
        Self { bands: hsl.bands }
    }
}

impl PipelineStage for HslStage {
    fn name(&self) -> &'static str {
        "hsl"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let bands = self.bands;
        image.map_pixels(|rgb| {
            let [l, a, b] = prophoto_to_oklab(rgb);
            let chroma = a.hypot(b);
            if chroma < EPS {
                return rgb; // neutral — nothing to band-match
            }
            let hue = {
                let d = b.atan2(a).to_degrees();
                if d < 0.0 {
                    d + 360.0
                } else {
                    d
                }
            };

            let w = band_weights(hue);
            let (mut dh, mut sat, mut lum) = (0.0, 0.0, 0.0);
            for i in 0..8 {
                dh += w[i] * bands[i].hue;
                sat += w[i] * bands[i].saturation;
                lum += w[i] * bands[i].luminance;
            }

            // Fade the whole adjustment out as chroma approaches zero, where hue is noise.
            let t = (chroma / CHROMA_FADE).min(1.0);
            let strength = t * t * (3.0 - 2.0 * t); // smoothstep
            dh *= strength;
            sat *= strength;
            lum *= strength;

            let hue2 = (hue + dh * MAX_HUE_SHIFT_DEG / 100.0).to_radians();
            let chroma2 = (chroma * (1.0 + sat / 100.0)).max(0.0);
            let l2 = (l * (1.0 + lum / 200.0)).max(0.0);

            let out = oklab_to_prophoto([l2, chroma2 * hue2.cos(), chroma2 * hue2.sin()]);
            [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oklab hue (degrees) of a linear RGB triple.
    fn hue_of(rgb: [f32; 3]) -> f32 {
        let [_, a, b] = prophoto_to_oklab(rgb);
        let d = b.atan2(a).to_degrees();
        if d < 0.0 {
            d + 360.0
        } else {
            d
        }
    }

    /// Oklab chroma of a linear RGB triple.
    fn chroma_of(rgb: [f32; 3]) -> f32 {
        let [_, a, b] = prophoto_to_oklab(rgb);
        a.hypot(b)
    }

    fn apply_one(hsl: &Hsl, px: [f32; 3]) -> [f32; 3] {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, px);
        HslStage::new(hsl).apply(img).pixel(0, 0)
    }

    /// A moderately saturated color that stays in gamut under the test adjustments.
    /// A reddish test pixel, **in the working space**.
    ///
    /// ⚠ Authored as the sRGB colour `[0.30, 0.06, 0.05]` and converted, the way the golden
    /// charts are. Written straight in, these numbers would be read as ProPhoto — a more
    /// saturated colour whose Oklab chroma sits differently against `CHROMA_FADE`, which is
    /// what made the hue-rotation test read 22° instead of 30°.
    const RED_ISH: [f32; 3] = [0.185_624, 0.083_325, 0.055_396];
    /// A bluish test pixel, in the working space. sRGB source: `[0.05, 0.07, 0.30]`.
    const BLUE_ISH: [f32; 3] = [0.091_750, 0.074_512, 0.268_719];

    fn all_bands(f: impl Fn(&mut HslBand)) -> Hsl {
        let mut hsl = Hsl::default();
        for b in hsl.bands.iter_mut() {
            f(b);
        }
        hsl
    }

    #[test]
    fn default_is_identity() {
        let out = apply_one(&Hsl::default(), RED_ISH);
        for i in 0..3 {
            assert!(
                (out[i] - RED_ISH[i]).abs() < 1e-4,
                "channel {i}: {out:?} vs {RED_ISH:?}"
            );
        }
    }

    #[test]
    fn neutral_pixel_is_unchanged() {
        let hsl = all_bands(|b| {
            b.hue = 100.0;
            b.saturation = 100.0;
            b.luminance = 100.0;
        });
        let gray = [0.3, 0.3, 0.3];
        let out = apply_one(&hsl, gray);
        for i in 0..3 {
            assert!((out[i] - gray[i]).abs() < 1e-4, "channel {i}: {out:?}");
        }
    }

    #[test]
    fn band_centers_match_canonical_colors() {
        // The table must stay in sync with our own conversion, not an external source.
        let canonical: [[f32; 3]; 8] = [
            [1.0, 0.0, 0.0],                                  // Red      #FF0000
            [1.0, color::srgb_to_linear(128.0 / 255.0), 0.0], // Orange #FF8000
            [1.0, 1.0, 0.0],                                  // Yellow   #FFFF00
            [0.0, 1.0, 0.0],                                  // Green    #00FF00
            [0.0, 1.0, 1.0],                                  // Aqua     #00FFFF
            [0.0, 0.0, 1.0],                                  // Blue     #0000FF
            [color::srgb_to_linear(128.0 / 255.0), 0.0, 1.0], // Purple #8000FF
            [1.0, 0.0, 1.0],                                  // Magenta  #FF00FF
        ];
        for (i, rgb) in canonical.iter().enumerate() {
            // ⚠ The canonical colours are **sRGB** (#FF0000 and friends), so they have to be
            // converted into the working space before the hue is measured — exactly as the
            // golden charts are. Feeding sRGB numbers to a ProPhoto conversion measures a
            // different, more saturated colour.
            //
            // ⚠ `BAND_HUES` is unchanged by the working-space switch, and that is the point:
            // Oklab hue is absolute, so the hue of sRGB red is the hue of sRGB red however it
            // is expressed. If this table ever has to move, something is wrong with the
            // matrices rather than with the table.
            let expected = hue_of(color::srgb_to_prophoto(*rgb));
            assert!(
                (BAND_HUES[i] - expected).abs() < 1.0,
                "band {i} ({}): table {} vs computed {expected}",
                edit_model::HSL_BAND_NAMES[i],
                BAND_HUES[i]
            );
        }
    }

    #[test]
    fn weights_sum_to_one_around_the_circle() {
        let mut h = 0.0;
        while h < 360.0 {
            let sum: f32 = band_weights(h).iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "hue {h}: sum {sum}");
            h += 0.5;
        }
    }

    #[test]
    fn red_band_saturation_leaves_blue_alone() {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = 100.0; // Red
        let red_out = apply_one(&hsl, RED_ISH);
        assert!(
            chroma_of(red_out) > chroma_of(RED_ISH) * 1.2,
            "red chroma {} -> {}",
            chroma_of(RED_ISH),
            chroma_of(red_out)
        );
        let blue_out = apply_one(&hsl, BLUE_ISH);
        for i in 0..3 {
            assert!(
                (blue_out[i] - BLUE_ISH[i]).abs() < 1e-3,
                "blue channel {i}: {blue_out:?} vs {BLUE_ISH:?}"
            );
        }
    }

    #[test]
    fn uniform_band_saturation_equals_a_global_chroma_scale() {
        let hsl = all_bands(|b| b.saturation = 50.0);
        for px in [RED_ISH, BLUE_ISH, [0.2, 0.25, 0.08]] {
            let out = apply_one(&hsl, px);
            let expected = chroma_of(px) * 1.5;
            assert!(
                (chroma_of(out) - expected).abs() < expected * 0.02,
                "{px:?}: chroma {} vs expected {expected}",
                chroma_of(out)
            );
        }
    }

    #[test]
    fn hue_shift_rotates_by_thirty_degrees_at_full_slider() {
        let hsl = all_bands(|b| b.hue = 100.0);
        for px in [RED_ISH, BLUE_ISH] {
            let before = hue_of(px);
            let after = hue_of(apply_one(&hsl, px));
            let delta = (after - before + 360.0) % 360.0;
            assert!((delta - 30.0).abs() < 1.0, "{px:?}: rotated {delta}°");
        }
    }

    #[test]
    fn luminance_slider_brightens_and_darkens() {
        let mut up = Hsl::default();
        up.bands[5].luminance = 100.0; // Blue
        let mut down = Hsl::default();
        down.bands[5].luminance = -100.0;
        let l = |rgb: [f32; 3]| prophoto_to_oklab(rgb)[0];
        assert!(l(apply_one(&up, BLUE_ISH)) > l(BLUE_ISH) * 1.2);
        assert!(l(apply_one(&down, BLUE_ISH)) < l(BLUE_ISH) * 0.8);
    }

    /// xorshift32, so the noise below is decorrelated across channels.
    struct Rng(u32);
    impl Rng {
        fn next_signed(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    /// A 64x64 patch of `base` plus per-channel noise, and its sample standard deviation.
    fn noisy_patch(base: [f32; 3], amp: f32) -> ImageBuf {
        let mut rng = Rng(0x1234_5678);
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.set_pixel(
                    x,
                    y,
                    [
                        base[0] + amp * rng.next_signed(),
                        base[1] + amp * rng.next_signed(),
                        base[2] + amp * rng.next_signed(),
                    ],
                );
            }
        }
        img
    }

    fn sd(img: &ImageBuf) -> f32 {
        let d = img.data();
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        (d.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / d.len() as f32).sqrt()
    }

    #[test]
    fn near_neutral_noise_is_not_amplified() {
        // A near-neutral pixel's hue is pure sensor noise (chroma ~0.003 in Oklab), so
        // applying a band adjustment at full strength makes neighbouring pixels land in
        // different bands — visible speckle. The chroma taper must suppress that.
        let mut hsl = Hsl::default();
        hsl.bands[1].luminance = 60.0; // Orange
        let src = noisy_patch([0.55, 0.55, 0.55], 0.006);
        let before = sd(&src);
        let after = sd(&HslStage::new(&hsl).apply(src));
        assert!(
            after < before * 1.5,
            "noise sd {before} -> {after} ({}x amplification)",
            after / before
        );
    }

    #[test]
    fn saturated_colors_keep_full_strength() {
        // The chroma taper must not weaken adjustments on genuinely colored pixels.
        // Chroma rather than lightness: a x1.5 lightness boost at fixed chroma leaves sRGB
        // gamut and gets clamped, which would measure a shortfall that has nothing to do
        // with the taper.
        let mut hsl = Hsl::default();
        hsl.bands[5].saturation = 50.0; // Blue
        let out = apply_one(&hsl, BLUE_ISH);
        // BLUE_ISH sits between the Blue and Aqua centers, so only Blue's share of the
        // weight applies. Folding that in makes this a precise assertion that the chroma
        // taper leaves strength at 1 for a genuinely colored pixel.
        let share = band_weights(hue_of(BLUE_ISH))[5];
        let expected = chroma_of(BLUE_ISH) * (1.0 + share * 50.0 / 100.0);
        assert!(
            (chroma_of(out) - expected).abs() < expected * 0.02,
            "chroma {} vs expected {expected}",
            chroma_of(out)
        );
    }

    #[test]
    fn never_emits_negative_samples() {
        let hsl = all_bands(|b| {
            b.hue = -100.0;
            b.saturation = 100.0;
            b.luminance = 100.0;
        });
        for px in [RED_ISH, BLUE_ISH, [0.9, 0.1, 0.0], [0.01, 0.02, 0.0]] {
            let out = apply_one(&hsl, px);
            assert!(out.iter().all(|&v| v >= 0.0), "{px:?} -> {out:?}");
        }
    }
}
