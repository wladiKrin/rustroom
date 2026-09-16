use crate::spatial::{box_blur, guided_filter};
use crate::{PipelineStage, Rect, RenderScale};
// ⚠ The **ProPhoto** pair, not the sRGB one: this stage converts the working space in
// and out again, and mixing the two directions produces a smooth, plausible colour shift
// that is very hard to attribute later.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::NoiseReduction;
use rayon::prelude::*;

/// Guided-filter window radius, in **full-resolution** pixels: the scale of the noise
/// grain we are averaging out. Not user-exposed — Detail already exposes the quality
/// trade-off, and a second knob here buys nothing.
pub const LUM_RADIUS_FULL: f32 = 6.0;
/// Chroma blur radius at Smoothness 0 and 100, in full-resolution pixels. Far larger than
/// the luminance radius: chroma is genuinely low-frequency in real images, so a wide blur
/// costs almost no visible detail.
pub const COL_RADIUS_FULL: (f32, f32) = (8.0, 32.0);
/// `eps` at Detail 0 and Detail 100 — the local variance below which a neighbourhood
/// counts as noise rather than structure.
const EPS_AT_DETAIL_0: f32 = 1e-3;
const EPS_AT_DETAIL_100: f32 = 1e-5;
/// Two box passes approximate a Gaussian closely enough to avoid the cross-shaped
/// artifact a single box blur leaves, and cost nothing asymptotically.
pub const CHROMA_PASSES: usize = 2;

/// Convert a full-resolution radius to the resolution actually being rendered. Never
/// returns 0: that would silently turn the filter off at small preview scales.
fn scaled_radius(full_px: f32, scale: RenderScale) -> usize {
    ((full_px * scale.get()).round().max(0.0) as usize).max(1)
}

/// The guided filter's noise/structure threshold, from the Detail slider. Interpolated
/// geometrically because the quantity spans two decades.
pub fn eps_for_detail(detail: f32) -> f32 {
    let t = (detail / 100.0).clamp(0.0, 1.0);
    EPS_AT_DETAIL_0 * (EPS_AT_DETAIL_100 / EPS_AT_DETAIL_0).powf(t)
}

/// Chroma blur radius in full-resolution pixels, from the Smoothness slider.
pub fn chroma_radius_full(smoothness: f32) -> f32 {
    let t = (smoothness / 100.0).clamp(0.0, 1.0);
    COL_RADIUS_FULL.0 + (COL_RADIUS_FULL.1 - COL_RADIUS_FULL.0) * t
}

/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// ⚠⚠ The **max** of the two halves, not their sum: both are computed from the same Oklab
/// conversion of the same source and written into different channels, so neither composes with
/// the other. (Contrast `sharpening::max_kernel_radius`, where the mask blurs `|d|` on top of the
/// first blur and the two radii genuinely add.)
///
/// ⚠⚠ **The luminance half reaches `2r`, not `r`.** It is a self-guided filter: every window
/// statistic is a box blur of radius `r`, and `mean_slope`/`mean_offset` blur those again. That is
/// `local_contrast::kernel_reach`'s rule, and forgetting it is what made this repo's recorded
/// ~110 px margin estimate for M8.7 too small by 1.6x.
///
/// ⚠ An inactive half contributes nothing, so a photo carrying only luminance NR does not keep a
/// 32 px margin for a chroma blur that never runs. That mirrors [`NoiseReduction::is_active`],
/// which is amount-only for the same reason.
///
/// Any renderer that crops a region before running this stage must keep at least this much margin
/// outside what it displays — see [`crate::region_plan`].
pub fn kernel_reach(params: &NoiseReduction, scale: RenderScale) -> usize {
    let lum = if params.luminance > 0.0 {
        2 * scaled_radius(LUM_RADIUS_FULL, scale)
    } else {
        0
    };
    let col = if params.color > 0.0 {
        // ⚠⚠ **`CHROMA_PASSES` passes, not one.** The chroma blur is `box_blur` applied twice, so
        // its reach is `passes * radius`. Forgetting this is the same species of error as the
        // recorded ~110 px margin estimate for M8.7, one level down — and it was caught by
        // `a_chain_with_a_local_agrees_to_within_a_rounding` rather than by
        // review, because an under-sized margin shows only as a wrong band at a region's edge.
        CHROMA_PASSES * scaled_radius(chroma_radius_full(params.smoothness), scale)
    } else {
        0
    };
    lum.max(col)
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

    /// Produce only `out` of this stage's output, from a margined region of its input.
    ///
    /// ⚠⚠ **Byte-identical to the whole-buffer path for every pixel in `out`, and the margin is
    /// what makes that true rather than approximately true.** [`crate::region_plan`] grows the
    /// request by [`kernel_reach`] on every side, so a pixel inside `out` reads only samples that
    /// are inside the buffer — and those samples hold exactly what the whole image holds there.
    /// The box blur's window shrinks only at the *buffer's* own edge, which `out` is inset from by
    /// construction.
    ///
    /// ⚠ It therefore fails **loudly** rather than clamping when `out` is not inside the buffer:
    /// that means some `input_rect_for` under-asked, and the symptom would otherwise be a subtly
    /// wrong band at the region's edge rather than an error.
    ///
    /// ⚠ The clone is of the *margined region*, not the frame — far smaller than the buffer this
    /// exists to avoid processing, which is the whole point.
    fn apply_region(&self, src: &crate::RegionSrc<'_>, out: Rect) -> Option<ImageBuf> {
        if out.w == 0 || out.h == 0 {
            return None;
        }
        let ox = out.x.checked_sub(src.origin.0)?;
        let oy = out.y.checked_sub(src.origin.1)?;
        if ox + out.w > src.buf.width() || oy + out.h > src.buf.height() {
            return None;
        }
        let done = self.apply(src.buf.clone());
        Some(done.crop(ox, oy, out.w, out.h))
    }

    /// ⚠ Grows by [`kernel_reach`] on every side. This stage preserves dimensions, so `out` and
    /// the result are in the same space and no `output_dims` resolution is needed.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
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
                let lab = prophoto_to_oklab([px[0], px[1], px[2]]);
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
                let rgb = oklab_to_prophoto([l, a, b]);
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
    #[test]
    fn the_kernel_reach_is_the_wider_of_the_two_halves() {
        // Luminance is a self-guided filter at radius 6, and a guided base reaches 2r — the rule
        // `local_contrast::kernel_reach` records. Chroma is a plain box blur at 8..32, so it
        // reaches its own radius. At smoothness 0 chroma is 8 and luminance's 12 wins; by
        // smoothness 50 the chroma window is 20 and it wins outright.
        let at = |lum: f32, col: f32, smooth: f32| {
            super::kernel_reach(
                &super::NoiseReduction {
                    luminance: lum,
                    color: col,
                    detail: 50.0,
                    smoothness: smooth,
                },
                super::RenderScale::FULL,
            )
        };
        assert_eq!(at(50.0, 0.0, 0.0), 12, "luminance only: 2 x 6");
        assert_eq!(
            at(0.0, 50.0, 0.0),
            16,
            "chroma only, smoothness 0: 2 passes x 8"
        );
        assert_eq!(
            at(0.0, 50.0, 100.0),
            64,
            "chroma only, smoothness 100: 2 passes x 32"
        );
        assert_eq!(
            at(50.0, 50.0, 0.0),
            16,
            "both, and chroma is already wider at smoothness 0: 2 passes x 8 beats 2 x 6"
        );
        assert_eq!(
            at(50.0, 50.0, 50.0),
            40,
            "both, and chroma is wider at smoothness 50"
        );
    }

    #[test]
    fn an_inactive_half_contributes_no_reach() {
        // ⚠ The discriminating case: an implementation that always took the max of both radii
        // would keep a 32 px margin for a photo carrying no chroma NR at all.
        let p = super::NoiseReduction {
            luminance: 50.0,
            color: 0.0,
            detail: 50.0,
            smoothness: 100.0,
        };
        assert_eq!(
            super::kernel_reach(&p, super::RenderScale::FULL),
            12,
            "chroma is off, so its 32 must not count"
        );
        let q = super::NoiseReduction {
            luminance: 0.0,
            color: 0.0,
            detail: 50.0,
            smoothness: 100.0,
        };
        assert_eq!(
            super::kernel_reach(&q, super::RenderScale::FULL),
            0,
            "wholly inactive reaches nothing"
        );
    }

    #[test]
    fn the_reach_follows_the_render_scale() {
        // ⚠⚠ At RenderScale::FULL the scaling is the identity, so every test above is
        // structurally blind to the scale being dropped — the exact blind spot
        // `the_radii_follow_the_render_scale` was written for in M8.3q. This is the only test
        // here that can see it.
        let p = super::NoiseReduction {
            luminance: 0.0,
            color: 50.0,
            detail: 50.0,
            smoothness: 100.0,
        };
        assert_eq!(super::kernel_reach(&p, super::RenderScale::FULL), 64);
        assert_eq!(super::kernel_reach(&p, super::RenderScale::new(0.5)), 32);
    }

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
        noisy_rect(64, 64, base, amp)
    }

    /// A `w`x`h` patch of `base` plus per-channel noise.
    ///
    /// ⚠ The RNG is seeded identically to `noisy_patch`, so a region comparison is between two
    /// runs over the SAME pixels rather than over two different noises.
    fn noisy_rect(w: usize, h: usize, base: [f32; 3], amp: f32) -> ImageBuf {
        let mut rng = Rng(0x1234_5678);
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // ⚠ The patch is authored in **sRGB** and converted into the working space,
                // exactly as the golden charts are. Writing these numbers straight in would
                // have them read as ProPhoto — a different, more saturated colour, whose
                // chroma noise this stage then handles differently.
                img.set_pixel(
                    x,
                    y,
                    color::srgb_to_prophoto([
                        base[0] + amp * rng.next_signed(),
                        base[1] + amp * rng.next_signed(),
                        base[2] + amp * rng.next_signed(),
                    ]),
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
            .map(|p| prophoto_to_oklab([p[0], p[1], p[2]])[0])
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
                let lab = prophoto_to_oklab([p[0], p[1], p[2]]);
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
            let lab = prophoto_to_oklab([p[0], p[1], p[2]]);
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
