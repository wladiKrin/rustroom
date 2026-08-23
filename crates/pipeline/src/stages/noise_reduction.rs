use crate::spatial::{box_blur, guided_filter};
use crate::{PipelineStage, RenderScale};
use color::{linear_srgb_to_oklab, oklab_to_linear_srgb};
use core_image::ImageBuf;
use edit_model::NoiseReduction;
use rayon::prelude::*;

/// Guided-filter window radius, in **full-resolution** pixels: the scale of the noise
/// grain we are averaging out. Not user-exposed — Detail already exposes the quality
/// trade-off, and a second knob here buys nothing.
const LUM_RADIUS_FULL: f32 = 6.0;
/// Chroma blur radius at Smoothness 0 and 100, in full-resolution pixels. Far larger than
/// the luminance radius: chroma is genuinely low-frequency in real images, so a wide blur
/// costs almost no visible detail.
const COL_RADIUS_FULL: (f32, f32) = (8.0, 32.0);
/// `eps` at Detail 0 and Detail 100 — the local variance below which a neighbourhood
/// counts as noise rather than structure.
const EPS_AT_DETAIL_0: f32 = 1e-3;
const EPS_AT_DETAIL_100: f32 = 1e-5;
/// Two box passes approximate a Gaussian closely enough to avoid the cross-shaped
/// artifact a single box blur leaves, and cost nothing asymptotically.
const CHROMA_PASSES: usize = 2;

/// Convert a full-resolution radius to the resolution actually being rendered. Never
/// returns 0: that would silently turn the filter off at small preview scales.
fn scaled_radius(full_px: f32, scale: RenderScale) -> usize {
    ((full_px * scale.get()).round().max(0.0) as usize).max(1)
}

/// The guided filter's noise/structure threshold, from the Detail slider. Interpolated
/// geometrically because the quantity spans two decades.
fn eps_for_detail(detail: f32) -> f32 {
    let t = (detail / 100.0).clamp(0.0, 1.0);
    EPS_AT_DETAIL_0 * (EPS_AT_DETAIL_100 / EPS_AT_DETAIL_0).powf(t)
}

/// Chroma blur radius in full-resolution pixels, from the Smoothness slider.
fn chroma_radius_full(smoothness: f32) -> f32 {
    let t = (smoothness / 100.0).clamp(0.0, 1.0);
    COL_RADIUS_FULL.0 + (COL_RADIUS_FULL.1 - COL_RADIUS_FULL.0) * t
}

/// Luminance and color noise reduction, in Oklab.
///
/// Runs early in the pipeline — before white balance — so it denoises the source rather
/// than fighting noise that Exposure, Shadows and the tone curve have already amplified.
/// Separating luminance from chroma is the point: chroma noise wants a wide, cheap blur,
/// while luminance noise needs an edge-aware filter, because that is where detail lives.
pub struct NoiseReductionStage {
    params: NoiseReduction,
    scale: RenderScale,
}

impl NoiseReductionStage {
    pub fn new(params: &NoiseReduction, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for NoiseReductionStage {
    fn name(&self) -> &'static str {
        "noise_reduction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }

        // One Oklab round trip serves both halves.
        let n = w * h;
        let mut lum = vec![0.0f32; n];
        let mut ca = vec![0.0f32; n];
        let mut cb = vec![0.0f32; n];
        lum.par_iter_mut()
            .zip(ca.par_iter_mut())
            .zip(cb.par_iter_mut())
            .zip(image.data().par_chunks_exact(3))
            .for_each(|(((l, a), b), px)| {
                let lab = linear_srgb_to_oklab([px[0], px[1], px[2]]);
                *l = lab[0];
                *a = lab[1];
                *b = lab[2];
            });

        if self.params.luminance > 0.0 {
            let r = scaled_radius(LUM_RADIUS_FULL, self.scale);
            let eps = eps_for_detail(self.params.detail);
            let filtered = guided_filter(&lum, &lum, w, h, r, eps);
            let s = (self.params.luminance / 100.0).clamp(0.0, 1.0);
            for (dst, q) in lum.iter_mut().zip(filtered) {
                *dst += (q - *dst) * s;
            }
        }

        if self.params.color > 0.0 {
            let r = scaled_radius(chroma_radius_full(self.params.smoothness), self.scale);
            let s = (self.params.color / 100.0).clamp(0.0, 1.0);
            for plane in [&mut ca, &mut cb] {
                let mut blurred = plane.clone();
                for _ in 0..CHROMA_PASSES {
                    blurred = box_blur(&blurred, w, h, r);
                }
                for (dst, q) in plane.iter_mut().zip(blurred) {
                    *dst += (q - *dst) * s;
                }
            }
        }

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .zip(lum.par_iter().zip(ca.par_iter()).zip(cb.par_iter()))
            .for_each(|(px, ((&l, &a), &b))| {
                let rgb = oklab_to_linear_srgb([l, a, b]);
                // Strong smoothing can push a pixel out of gamut; clamp at 0, the same
                // convention HslStage uses.
                px[0] = rgb[0].max(0.0);
                px[1] = rgb[1].max(0.0);
                px[2] = rgb[2].max(0.0);
            });
        ImageBuf::from_vec(w, h, data)
    }
}

#[cfg(test)]
mod tests {
    // Brings in NoiseReductionStage, RenderScale, ImageBuf, NoiseReduction, PipelineStage
    // (needed for `.apply`) and the Oklab conversions.
    use super::*;

    /// xorshift32, so the noise below is decorrelated across channels. (Deliberately a
    /// local copy of the HSL tests' helper: a 20-line deterministic fixture is not worth
    /// a shared test-support crate, and the two copies are free to diverge.)
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

    /// A 64x64 patch of `base` plus per-channel noise.
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

    /// Standard deviation of the image's Oklab L — what luminance NR is supposed to move.
    /// Raw RGB sd would be a weak proxy: most of a gray patch's RGB variance lives in the
    /// chroma directions, which luminance NR does not touch.
    fn l_sd(img: &ImageBuf) -> f32 {
        let ls: Vec<f32> = img
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| linear_srgb_to_oklab([p[0], p[1], p[2]])[0])
            .collect();
        let mean = ls.iter().sum::<f32>() / ls.len() as f32;
        (ls.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / ls.len() as f32).sqrt()
    }

    /// Standard deviation of the Oklab a and b samples pooled — what color NR moves.
    fn chroma_sd(img: &ImageBuf) -> f32 {
        let cs: Vec<f32> = img
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| {
                let lab = linear_srgb_to_oklab([p[0], p[1], p[2]]);
                [lab[1], lab[2]]
            })
            .collect();
        let mean = cs.iter().sum::<f32>() / cs.len() as f32;
        (cs.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cs.len() as f32).sqrt()
    }

    /// Mean Oklab a and b — the average color, which color NR must *not* move.
    fn mean_ab(img: &ImageBuf) -> (f32, f32) {
        let n = (img.width() * img.height()) as f32;
        let (mut sa, mut sb) = (0.0, 0.0);
        for p in img.data().as_chunks::<3>().0 {
            let lab = linear_srgb_to_oklab([p[0], p[1], p[2]]);
            sa += lab[1];
            sb += lab[2];
        }
        (sa / n, sb / n)
    }

    fn run(params: NoiseReduction, img: ImageBuf) -> ImageBuf {
        NoiseReductionStage::new(&params, RenderScale::FULL).apply(img)
    }

    #[test]
    fn zero_strengths_are_an_exact_identity() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let params = NoiseReduction {
            luminance: 0.0,
            detail: 0.0,
            color: 0.0,
            smoothness: 100.0,
        };
        assert_eq!(run(params, src.clone()), src);
    }

    #[test]
    fn luminance_strength_cuts_luminance_noise() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let before = l_sd(&src);
        let after = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(
            after < before * 0.5,
            "L sd {before} -> {after} ({}x)",
            after / before
        );
    }

    #[test]
    fn a_hard_edge_keeps_its_contrast() {
        // The guided filter's entire claim. A plain blur fails this; a botched `slope`
        // term fails this.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, if x < 32 { [0.05; 3] } else { [0.75; 3] });
            }
        }
        let out = run(
            NoiseReduction {
                luminance: 100.0,
                ..Default::default()
            },
            src.clone(),
        );
        let want = src.pixel(32, 20)[0] - src.pixel(31, 20)[0];
        let got = out.pixel(32, 20)[0] - out.pixel(31, 20)[0];
        assert!(got > want * 0.9, "edge contrast {want} -> {got}");
    }

    #[test]
    fn detail_slider_trades_smoothing_for_texture() {
        let src = noisy_patch([0.4, 0.4, 0.4], 0.02);
        let smooth = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                detail: 0.0,
                ..Default::default()
            },
            src.clone(),
        ));
        let preserve = l_sd(&run(
            NoiseReduction {
                luminance: 100.0,
                detail: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(
            preserve > smooth * 1.5,
            "detail 0 -> sd {smooth}, detail 100 -> sd {preserve}"
        );
    }

    #[test]
    fn color_strength_cuts_chroma_noise_without_moving_the_color() {
        let src = noisy_patch([0.30, 0.15, 0.10], 0.02);
        let (a0, b0) = mean_ab(&src);
        let before = chroma_sd(&src);
        let out = run(
            NoiseReduction {
                color: 100.0,
                ..Default::default()
            },
            src,
        );
        let after = chroma_sd(&out);
        assert!(
            after < before * 0.3,
            "chroma sd {before} -> {after} ({}x)",
            after / before
        );
        let (a1, b1) = mean_ab(&out);
        assert!((a1 - a0).abs() < 0.005 && (b1 - b0).abs() < 0.005);
    }

    #[test]
    fn smoothness_widens_the_chroma_blur() {
        // A chroma wave with a 32 px period: the small radius leaves much of it, the
        // large one (window wider than the image) flattens it.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let s = (x as f32 / 32.0 * std::f32::consts::TAU).sin();
                src.set_pixel(x, y, [0.3 + 0.1 * s, 0.3, 0.3 - 0.1 * s]);
            }
        }
        let narrow = chroma_sd(&run(
            NoiseReduction {
                color: 100.0,
                smoothness: 0.0,
                ..Default::default()
            },
            src.clone(),
        ));
        let wide = chroma_sd(&run(
            NoiseReduction {
                color: 100.0,
                smoothness: 100.0,
                ..Default::default()
            },
            src,
        ));
        assert!(wide < narrow, "smoothness 0 -> {narrow}, 100 -> {wide}");
    }

    #[test]
    fn a_noisy_gray_patch_goes_neutral_under_color_nr() {
        let src = noisy_patch([0.45, 0.45, 0.45], 0.02);
        let out = run(
            NoiseReduction {
                color: 100.0,
                smoothness: 100.0,
                ..Default::default()
            },
            src,
        );
        let (a, b) = mean_ab(&out);
        assert!(a.abs() < 2e-3 && b.abs() < 2e-3, "mean a {a}, b {b}");
        assert!(chroma_sd(&out) < 2e-3);
    }

    #[test]
    fn never_emits_negative_samples() {
        let src = noisy_patch([0.02, 0.01, 0.005], 0.02);
        let out = run(
            NoiseReduction {
                luminance: 100.0,
                detail: 0.0,
                color: 100.0,
                smoothness: 100.0,
            },
            src,
        );
        assert!(out.data().iter().all(|&v| v >= 0.0));
    }

    #[test]
    fn radii_scale_with_the_render_scale() {
        assert_eq!(scaled_radius(6.0, RenderScale::FULL), 6);
        assert_eq!(scaled_radius(6.0, RenderScale::new(0.25)), 2);
        // Never rounds down to zero — a zero radius would silently disable the filter.
        assert_eq!(scaled_radius(6.0, RenderScale::new(0.01)), 1);
    }

    #[test]
    fn eps_spans_two_decades_geometrically() {
        assert!((eps_for_detail(0.0) - 1e-3).abs() < 1e-9);
        assert!((eps_for_detail(100.0) - 1e-5).abs() < 1e-9);
        // Geometric midpoint, not arithmetic: the quantity is a variance spanning two
        // decades, so a linear slider would be unusable at one end.
        assert!((eps_for_detail(50.0) - 1e-4).abs() < 1e-7);
    }

    #[test]
    fn chroma_radius_grows_with_smoothness() {
        assert_eq!(chroma_radius_full(0.0), 8.0);
        assert_eq!(chroma_radius_full(100.0), 32.0);
        assert_eq!(chroma_radius_full(50.0), 20.0);
    }
}
