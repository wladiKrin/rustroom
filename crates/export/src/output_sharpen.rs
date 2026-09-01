//! Sharpening for the output medium, applied to the resized file rather than to the photograph.
//!
//! ⚠⚠ **NOT A PIPELINE STAGE, AND THE REASON IS NOT MERELY THAT ANOTHER TRACK OWNS `stages/`.**
//! `export_one` resizes **before** the pipeline, deliberately: `downscale_to` averages *linear*
//! samples, where averaging is meaningful, and every stage then runs on fewer pixels. The
//! consequence CLAUDE.md already records is that every radius scales with the output, so a
//! resized export preserves the *look* of Detail rather than its pixel radius — it is not a
//! downsampled full-resolution export. **This is the step that compensates for that.** Its
//! radius is fixed in *output* pixels precisely because the resample's softening is a property
//! of the delivered file, not of the negative.
//!
//! ⚠⚠ **IT WORKS ON THE DISPLAY-ENCODED BUFFER, WHERE `SharpenStage` WORKS IN LINEAR LIGHT, AND
//! THAT DIVERGENCE IS DELIBERATE.** Capture sharpening measures the photograph, so it must be
//! linear or it is measuring the transfer curve. Output sharpening is applied to the *file*, in
//! the space the file is written in — which is what Lightroom, Capture One and Photokit
//! Sharpener all do, and for a good reason: sharpening in linear light produces halos that are
//! brighter than the eye expects, because equal linear excursions are not equal perceptual ones.
//!
//! ⚠ What is kept verbatim from `SharpenStage` is the *technique*, which is the half CLAUDE.md
//! argues for: a high-pass on an achromatic channel, re-applied as a **scalar RGB gain**. That
//! buys no colour fringing on a chromatic edge, hue and saturation ratios preserved exactly, and
//! no Oklab round trip. The near-black guard is load-bearing here for the identical reason.

use core_image::ImageBuf;
use pipeline::spatial::gaussian_blur;
use serde::{Deserialize, Serialize};

/// Sharpening for the output medium, applied after the resize and before the encode.
///
/// ⚠ `0` is off, which is what makes the default inert — the rule `Vignette`, `Sharpening` and
/// `NoiseReduction` already follow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSharpening {
    /// 0..=100. `0` means nothing runs at all.
    pub amount: u8,
}

impl OutputSharpening {
    pub fn is_active(&self) -> bool {
        self.amount > 0
    }
}

/// Blur radius, as sigma in **output** pixels.
///
/// ⚠ Fixed rather than scaled, and that is the whole difference from `SharpenStage`, whose sigma
/// is in full-resolution pixels times the render scale. Output sharpening exists to put back the
/// edge the resample took, and the resample's kernel is a property of the destination grid: an
/// 800 px JPEG and a 6000 px TIFF want the same sigma *in their own pixels*.
///
/// 0.8 is the conventional screen-output radius — small enough that the halo is under a pixel at
/// ordinary viewing distances, large enough to survive an 8-bit quantization.
const SIGMA: f32 = 0.8;

/// Strength at amount 100. ⚠ Lower than `SharpenStage`'s 1.5: this runs *on top of* whatever
/// capture sharpening the photograph already carries, and stacking two unsharp masks at full
/// strength is how an export comes out crunchy.
const MAX_AMOUNT: f32 = 0.8;

/// Near-black guard for the gain ratio. **Load-bearing**: without it a dark pixel beside a
/// bright edge has `luma ~ 0` and the gain explodes.
const LUMA_EPS: f32 = 1e-3;

/// Caps a halo in near-black regions.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma coefficients, applied to **display-encoded** components.
///
/// ⚠⚠ **THIS IS Y′ (LUMA), NOT Y (LUMINANCE), AND THE DISTINCTION IS WHY THIS IS NOT ANOTHER OF
/// THE SIX LOCAL COPIES CLAUDE.md WARNS ABOUT.** Those were `color::luminance` written out by
/// hand, and they all became wrong the day the working space stopped being sRGB, because
/// luminance is a physical quantity that depends on the primaries. Luma is *defined* on
/// gamma-corrected components, and this buffer is gamma-corrected — `color::luminance` here
/// would be applying the working space's weights to data that has left it.
///
/// ⚠ The weights differ slightly for P3, Adobe RGB and ProPhoto output. That is accepted rather
/// than dispatched on: the gain is a **ratio**, so hue and saturation are preserved exactly
/// whatever weights are chosen — the choice only shifts *how much* a given pixel is sharpened.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luma(px: &[f32; 3]) -> f32 {
    px[0] * LUMA[0] + px[1] * LUMA[1] + px[2] * LUMA[2]
}

/// Sharpen a display-encoded image in place. A no-op at amount 0.
pub fn apply(image: &mut ImageBuf, params: &OutputSharpening) {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 || !params.is_active() {
        return;
    }
    // ⚠ `as_chunks`, not `chunks_exact(3)` — it yields `[f32; 3]`, so the optimiser knows the
    // length. Clippy's `chunks_exact_to_as_chunks` insists, and CLAUDE.md records that the
    // rewrite is not merely cosmetic: it measured ~9% on noise reduction.
    let l: Vec<f32> = image.data().as_chunks::<3>().0.iter().map(luma).collect();
    // ⚠ Reused read-only from `pipeline::spatial` rather than written again here. A second
    // Gaussian in this repository is a second set of border rules to keep in step.
    let blurred = gaussian_blur(&l, w, h, SIGMA);
    let k = f32::from(params.amount.min(100)) / 100.0 * MAX_AMOUNT;

    for (i, px) in image
        .data_mut()
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .enumerate()
    {
        let d = l[i] - blurred[i];
        let gain = (1.0 + k * d / l[i].max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
        px[0] *= gain;
        px[1] *= gain;
        px[2] *= gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hard vertical edge at x = 16, in display-encoded values.
    fn hard_edge() -> ImageBuf {
        let mut img = ImageBuf::new(32, 8);
        for y in 0..8 {
            for x in 0..32 {
                img.set_pixel(x, y, if x < 16 { [0.30; 3] } else { [0.70; 3] });
            }
        }
        img
    }

    #[test]
    fn amount_zero_leaves_the_image_bit_identical() {
        // ⚠ **Recorded honestly: deleting the `is_active` early return is INERT.** At amount 0
        // the strength `k` is exactly 0, so the gain is exactly 1.0 and every multiply is the
        // identity — the guard is a cheap path, not a correctness one. No test can distinguish
        // the two, and inventing one that appeared to would be worse than saying so.
        let base = hard_edge();
        let mut img = base.clone();
        apply(&mut img, &OutputSharpening::default());
        assert_eq!(img.data(), base.data());
    }

    #[test]
    fn it_steepens_the_edge_and_leaves_the_flat_regions_alone() {
        let base = hard_edge();
        let mut img = base.clone();
        apply(&mut img, &OutputSharpening { amount: 100 });
        // Overshoot on the bright side of the edge, undershoot on the dark side.
        assert!(
            img.pixel(16, 4)[0] > base.pixel(16, 4)[0],
            "no overshoot: {} vs {}",
            img.pixel(16, 4)[0],
            base.pixel(16, 4)[0]
        );
        assert!(
            img.pixel(15, 4)[0] < base.pixel(15, 4)[0],
            "no undershoot: {} vs {}",
            img.pixel(15, 4)[0],
            base.pixel(15, 4)[0]
        );
        // ⚠ Far from the edge nothing may move at all. A stage that lifts the whole frame is
        // an exposure change wearing a sharpening label, and the overshoot test alone cannot
        // see it.
        for x in [0, 1, 30, 31] {
            assert!(
                (img.pixel(x, 4)[0] - base.pixel(x, 4)[0]).abs() < 1e-6,
                "the flat region at x={x} moved"
            );
        }
    }

    #[test]
    fn a_neutral_pixel_stays_neutral_and_a_coloured_one_keeps_its_ratios() {
        // ⚠ The property the scalar-gain technique exists for, and the only test that can see a
        // per-channel implementation: sharpening each channel against its own high-pass fringes
        // a chromatic edge, and the edge tests above cannot tell the two apart on grey.
        let mut img = ImageBuf::new(32, 4);
        for y in 0..4 {
            for x in 0..32 {
                img.set_pixel(
                    x,
                    y,
                    if x < 16 {
                        [0.2, 0.4, 0.1]
                    } else {
                        [0.6, 0.9, 0.3]
                    },
                );
            }
        }
        let base = img.clone();
        apply(&mut img, &OutputSharpening { amount: 100 });
        for x in [14usize, 15, 16, 17] {
            let (a, b) = (base.pixel(x, 2), img.pixel(x, 2));
            let gain = b[0] / a[0];
            assert!((b[1] / a[1] - gain).abs() < 1e-5, "x={x}: green drifted");
            assert!((b[2] / a[2] - gain).abs() < 1e-5, "x={x}: blue drifted");
            assert!((gain - 1.0).abs() > 1e-4, "x={x}: nothing happened");
        }
    }

    #[test]
    fn the_near_black_guard_bounds_the_gain() {
        // A near-black frame with one bright pixel: without `LUMA_EPS` the ratio explodes.
        let mut img = ImageBuf::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set_pixel(x, y, [0.0; 3]);
            }
        }
        img.set_pixel(8, 8, [1.0; 3]);
        apply(&mut img, &OutputSharpening { amount: 100 });
        for v in img.data() {
            assert!(v.is_finite(), "a non-finite sample escaped the guard");
            assert!(*v <= GAIN_MAX, "{v} exceeds the gain cap");
        }
    }

    #[test]
    fn stronger_amounts_sharpen_more() {
        let base = hard_edge();
        let overshoot = |amount: u8| {
            let mut img = base.clone();
            apply(&mut img, &OutputSharpening { amount });
            img.pixel(16, 4)[0] - base.pixel(16, 4)[0]
        };
        let (half, full) = (overshoot(50), overshoot(100));
        assert!(half > 0.0 && full > half, "half {half}, full {full}");
    }

    #[test]
    fn the_radius_does_not_follow_the_image_size() {
        // ⚠⚠ The property that separates output sharpening from capture sharpening, and the one
        // no other test here can see: the same amount on a 2x larger frame must reach the same
        // number of **pixels**, not the same fraction of the picture. A sigma that scaled with
        // the image would widen the affected band.
        let band = |w: usize| {
            let mut img = ImageBuf::new(w, 4);
            for y in 0..4 {
                for x in 0..w {
                    img.set_pixel(x, y, if x < w / 2 { [0.3; 3] } else { [0.7; 3] });
                }
            }
            let base = img.clone();
            apply(&mut img, &OutputSharpening { amount: 100 });
            (0..w)
                .filter(|&x| (img.pixel(x, 2)[0] - base.pixel(x, 2)[0]).abs() > 1e-5)
                .count()
        };
        assert_eq!(
            band(64),
            band(256),
            "the affected band widened with the image"
        );
    }
}
