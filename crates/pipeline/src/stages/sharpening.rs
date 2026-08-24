use crate::spatial::{gaussian_blur, kernel_radius};
use crate::{PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::Sharpening;
use rayon::prelude::*;

/// Radius slider range, as sigma in **full-resolution pixels**.
///
/// Public because the app's region margin has to exceed the kernel this implies — see
/// [`max_kernel_radius`] and the invariant asserted in `app::view_region`.
pub const RADIUS_RANGE: (f32, f32) = (0.5, 3.0);

/// Sharpening strength at Amount 100.
const MAX_AMOUNT: f32 = 1.5;
/// Luma excursion above which Detail starts suppressing.
const HALO_KNEE: f32 = 0.05;
/// Sigma of the blur applied to `|d|` so the mask is smooth rather than speckled, in
/// full-resolution pixels. Scaled by `RenderScale` like the main sigma, or the mask would
/// change character between the fit and native render modes.
const MASK_SIGMA: f32 = 2.0;
/// Edge strength at which Masking 100 still sharpens fully.
const MASK_MAX: f32 = 0.08;
/// Near-black guard for the ratio. Load-bearing: without it a dark pixel beside a bright
/// edge has `luma ~ 0` and the gain explodes.
const LUMA_EPS: f32 = 1e-3;
/// Caps a sharpening halo in near-black regions at +2 stops.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma, in linear light.
fn luma(px: &[f32]) -> f32 {
    // ⚠ Delegates to `color::luminance`, which carries the **working space's** weights. This
    // used to be a local copy of the Rec.709 triple — one of six, all of which became wrong the
    // day the working space stopped being sRGB.
    color::luminance([px[0], px[1], px[2]])
}

/// Clamped Hermite smoothstep. Same form as the one in `stages/vignette.rs`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The blur sigma actually used, in rendered pixels.
fn sigma_for(params: &Sharpening, scale: RenderScale) -> f32 {
    params.radius.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1) * scale.get()
}

/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// The two Gaussians compose: `d` reaches `kernel_radius(sigma)`, and the mask blurs `|d|`
/// a further `kernel_radius(MASK_SIGMA · scale)` on top of that. Any renderer that crops a
/// region before running this stage must keep at least this many pixels of margin outside
/// what it displays, or the region's edge band is computed from neighbours that do not
/// exist. `app::view_region` asserts exactly that against `REGION_MARGIN`.
pub fn max_kernel_radius(radius_px: f32, scale: RenderScale) -> usize {
    let s = scale.get();
    kernel_radius(radius_px.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1) * s)
        + kernel_radius(MASK_SIGMA * s)
}

/// Unsharp-mask sharpening on Rec.709 luma, applied back as an RGB gain.
///
/// Working on luma and re-applying as a ratio means no colour fringing on a chromatic edge,
/// hue and saturation ratios preserved exactly, and no Oklab round trip — the cost that
/// dominates `HslStage` and `NoiseReductionStage`.
///
/// **Detail** suppresses halos where the local contrast is already high: at 100 `supp = 1`
/// and this is a pure unsharp mask; at 0, `supp -> 1` for small `|d|` (fine texture keeps
/// full sharpening) and `-> 0` for large `|d|`. Our own model, not Adobe's — defensible and
/// monotone rather than a clone, and genuinely distinct from Masking, which gates on *where*
/// rather than *how strong*.
///
/// Runs after the vignette, so darkened corners sharpen proportionally less in absolute
/// terms — correct for a ratio-based model.
pub struct SharpenStage {
    params: Sharpening,
    scale: RenderScale,
}

impl SharpenStage {
    pub fn new(params: &Sharpening, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for SharpenStage {
    fn name(&self) -> &'static str {
        "sharpening"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let n = w * h;

        let l: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();
        let blurred = gaussian_blur(&l, w, h, sigma_for(&self.params, self.scale));
        let d: Vec<f32> = l.iter().zip(&blurred).map(|(a, b)| a - b).collect();

        // Masking gates on *where* to sharpen. At 0 the whole branch is skipped: that is
        // both the cheap path and what avoids `smoothstep(0, 0, ..)` being degenerate.
        let masking = self.params.masking.clamp(0.0, 100.0);
        let mask: Option<Vec<f32>> = (masking > 0.0).then(|| {
            let magnitude: Vec<f32> = d.iter().map(|v| v.abs()).collect();
            let edge = gaussian_blur(&magnitude, w, h, MASK_SIGMA * self.scale.get());
            let hi = MASK_MAX * masking / 100.0;
            edge.iter().map(|&e| smoothstep(0.0, hi, e)).collect()
        });

        let s = (self.params.detail / 100.0).clamp(0.0, 1.0);
        let k = (self.params.amount / 100.0).clamp(0.0, 1.0) * MAX_AMOUNT;

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .enumerate()
            .zip(image.data().par_chunks_exact(3))
            .for_each(|((i, o), px)| {
                let dv = d[i];
                let supp = s + (1.0 - s) / (1.0 + (dv.abs() / HALO_KNEE).powi(2));
                let m = mask.as_ref().map_or(1.0, |mk| mk[i]);
                let delta = k * dv * supp * m;
                let gain = (1.0 + delta / l[i].max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
                o[0] = px[0] * gain;
                o[1] = px[1] * gain;
                o[2] = px[2] * gain;
            });
        ImageBuf::from_vec(w, h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64x64 image split by a hard vertical edge at x = 32.
    fn hard_edge() -> ImageBuf {
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.set_pixel(x, y, if x < 32 { [0.10; 3] } else { [0.80; 3] });
            }
        }
        img
    }

    fn run(params: Sharpening, img: ImageBuf) -> ImageBuf {
        SharpenStage::new(&params, RenderScale::FULL).apply(img)
    }

    fn sharpen(amount: f32) -> Sharpening {
        Sharpening {
            amount,
            detail: 100.0,
            ..Default::default()
        }
    }

    #[test]
    fn amount_zero_is_an_exact_identity() {
        let src = hard_edge();
        assert_eq!(run(Sharpening::default(), src.clone()), src);
    }

    #[test]
    fn a_flat_region_is_untouched() {
        // d = 0 everywhere, so the gain is 1 and nothing moves.
        let mut src = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                src.set_pixel(x, y, [0.42, 0.30, 0.18]);
            }
        }
        let out = run(sharpen(100.0), src.clone());
        for (a, b) in out.data().iter().zip(src.data()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn a_hard_edge_gains_contrast() {
        // The entire point of the stage.
        let src = hard_edge();
        let out = run(sharpen(100.0), src.clone());
        let before = src.pixel(32, 32)[1] - src.pixel(31, 32)[1];
        let after = out.pixel(32, 32)[1] - out.pixel(31, 32)[1];
        assert!(after > before * 1.1, "edge contrast {before} -> {after}");
    }

    #[test]
    fn hue_ratios_are_preserved() {
        // Applying the correction as a scalar RGB gain is what buys this: no colour
        // fringing on a chromatic edge, and no Oklab round trip to pay for.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 0.30 } else { 0.70 };
                src.set_pixel(x, y, [v, v * 0.6, v * 0.35]);
            }
        }
        let out = run(sharpen(60.0), src.clone());
        let mut moved = 0usize;
        for (o, s) in out
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(src.data().as_chunks::<3>().0)
        {
            if o[0] <= 0.0 {
                // A gain clamped to 0 is still a scalar multiply; there is simply no ratio
                // left to compare.
                continue;
            }
            assert!((o[1] / o[0] - s[1] / s[0]).abs() < 1e-4);
            assert!((o[2] / o[0] - s[2] / s[0]).abs() < 1e-4);
            if (o[0] - s[0]).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing was sharpened, so nothing was tested");
    }

    #[test]
    fn radius_scales_with_the_render_scale() {
        // A full-resolution radius rendered at 1/4 scale is a quarter-size blur, so the
        // preview approximates the export rather than contradicting it.
        let p = Sharpening {
            amount: 100.0,
            radius: 2.0,
            ..Default::default()
        };
        assert_eq!(sigma_for(&p, RenderScale::FULL), 2.0);
        assert_eq!(sigma_for(&p, RenderScale::new(0.25)), 0.5);
        // Out-of-range radii are clamped rather than trusted.
        let wide = Sharpening { radius: 99.0, ..p };
        assert_eq!(sigma_for(&wide, RenderScale::FULL), RADIUS_RANGE.1);
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        let src = hard_edge();
        let out = run(
            Sharpening {
                amount: 100.0,
                radius: 3.0,
                detail: 100.0,
                masking: 0.0,
            },
            src,
        );
        assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    /// A 64x64 image of fine, low-amplitude texture: a 4 px-period sine on a mid gray.
    fn fine_texture() -> ImageBuf {
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = 0.40 + 0.008 * (x as f32 / 4.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// A 64x64 flat gray patch under low-amplitude per-channel noise.
    fn noisy_flat() -> ImageBuf {
        let mut state: u32 = 0x9E37_79B9;
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ((state as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.01
        };
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = 0.40 + noise();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// Mean absolute change over the image interior (avoiding the replicated border).
    fn mean_abs_change(a: &ImageBuf, b: &ImageBuf) -> f32 {
        let (mut sum, mut count) = (0.0f32, 0usize);
        for y in 12..a.height() - 12 {
            for x in 12..a.width() - 12 {
                sum += (a.pixel(x, y)[1] - b.pixel(x, y)[1]).abs();
                count += 1;
            }
        }
        sum / count as f32
    }

    #[test]
    fn detail_zero_suppresses_a_strong_edge_far_more_than_fine_texture() {
        // The claim that makes Detail a distinct control: it gates on *how strong* the
        // local excursion is, so halos around a hard edge go while texture survives.
        //
        // Sampled at x = 32, the pixel *on* the edge, where |d| is ~0.15 in luma — three
        // times HALO_KNEE. One pixel further out |d| has already collapsed to ~0.03 and
        // Detail correctly barely engages, so the sample point matters.
        let strong = |detail: f32| {
            let src = hard_edge();
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail,
                    ..Default::default()
                },
                src.clone(),
            );
            (out.pixel(32, 32)[1] - src.pixel(32, 32)[1]).abs()
        };
        let texture = |detail: f32| {
            let src = fine_texture();
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail,
                    ..Default::default()
                },
                src.clone(),
            );
            mean_abs_change(&out, &src)
        };
        let edge_ratio = strong(0.0) / strong(100.0);
        let texture_ratio = texture(0.0) / texture(100.0);
        assert!(edge_ratio < 0.2, "edge kept {edge_ratio} of its sharpening");
        assert!(
            texture_ratio > 0.8,
            "texture kept only {texture_ratio} of its sharpening"
        );
    }

    #[test]
    fn detail_100_is_a_pure_unsharp_mask() {
        // supp = 1 identically, so the result must not depend on HALO_KNEE at all.
        let src = hard_edge();
        let p = Sharpening {
            amount: 80.0,
            detail: 100.0,
            ..Default::default()
        };
        let out = run(p, src.clone());
        let l: Vec<f32> = src
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| luma(p))
            .collect();
        let blur = gaussian_blur(&l, 64, 64, sigma_for(&p, RenderScale::FULL));
        let (x, y) = (32usize, 32usize);
        let i = y * 64 + x;
        let expect_luma = l[i] + 0.8 * MAX_AMOUNT * (l[i] - blur[i]);
        let got_luma = luma(&out.pixel(x, y));
        assert!(
            (got_luma - expect_luma).abs() < 1e-4,
            "{got_luma} vs {expect_luma}"
        );
    }

    #[test]
    fn masking_spares_a_noisy_flat_patch_while_an_edge_still_sharpens() {
        // The pairing that justifies the control at all: one number must do both.
        let masked = |img: ImageBuf, masking: f32| {
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail: 100.0,
                    masking,
                    ..Default::default()
                },
                img.clone(),
            );
            (out, img)
        };

        let (flat_on, flat_src) = masked(noisy_flat(), 80.0);
        let (flat_off, _) = masked(noisy_flat(), 0.0);
        let kept_in_flat =
            mean_abs_change(&flat_on, &flat_src) / mean_abs_change(&flat_off, &flat_src);
        assert!(
            kept_in_flat < 0.2,
            "masking left {kept_in_flat} of the noise sharpening"
        );

        // On the edge pixel the blurred |d| reaches ~0.066, just past Masking 80's
        // threshold of 0.064, so the gate is effectively fully open there.
        let (edge_on, edge_src) = masked(hard_edge(), 80.0);
        let (edge_off, _) = masked(hard_edge(), 0.0);
        let d_on = (edge_on.pixel(32, 32)[1] - edge_src.pixel(32, 32)[1]).abs();
        let d_off = (edge_off.pixel(32, 32)[1] - edge_src.pixel(32, 32)[1]).abs();
        assert!(
            d_on > d_off * 0.7,
            "the edge lost sharpening too: {d_on} vs {d_off}"
        );
    }

    #[test]
    fn masking_zero_skips_the_gate_entirely() {
        // Not just "mask == 1 numerically": smoothstep(0, 0, ..) is degenerate, so the
        // branch has to be skipped rather than evaluated with a zero-width ramp.
        let src = hard_edge();
        let base = Sharpening {
            amount: 70.0,
            masking: 0.0,
            ..Default::default()
        };
        let out = run(base, src.clone());
        assert!(out.data().iter().all(|v| v.is_finite()));
        assert_ne!(out, src);
    }

    #[test]
    fn max_kernel_radius_covers_both_gaussians() {
        // radius 3 -> ceil(9) = 9, plus the mask's ceil(3 * 2) = 6.
        assert_eq!(max_kernel_radius(RADIUS_RANGE.1, RenderScale::FULL), 15);
        // Halving the render scale halves both sigmas.
        assert_eq!(max_kernel_radius(RADIUS_RANGE.1, RenderScale::new(0.5)), 8);
        // Monotone in the render scale, so a smaller preview never needs more margin.
        assert!(
            max_kernel_radius(RADIUS_RANGE.1, RenderScale::new(0.25))
                <= max_kernel_radius(RADIUS_RANGE.1, RenderScale::FULL)
        );
    }

    #[test]
    fn the_near_black_guard_bounds_the_gain() {
        // Without LUMA_EPS a near-black pixel beside a bright edge divides by ~0 and the
        // ratio explodes. GAIN_MAX then caps the halo at +2 stops.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, if x < 32 { [1e-6; 3] } else { [1.0; 3] });
            }
        }
        let out = run(sharpen(100.0), src.clone());
        for (o, s) in out.data().iter().zip(src.data()) {
            assert!(o.is_finite(), "non-finite sample {o}");
            assert!(*o <= s * GAIN_MAX + 1e-6, "{o} exceeds {s} * {GAIN_MAX}");
        }
    }
}
