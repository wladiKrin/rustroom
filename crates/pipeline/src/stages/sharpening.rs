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
#[allow(dead_code)]
const MASK_SIGMA: f32 = 2.0;
/// Edge strength at which Masking 100 still sharpens fully.
#[allow(dead_code)]
const MASK_MAX: f32 = 0.08;
/// Near-black guard for the ratio. Load-bearing: without it a dark pixel beside a bright
/// edge has `luma ~ 0` and the gain explodes.
const LUMA_EPS: f32 = 1e-3;
/// Caps a sharpening halo in near-black regions at +2 stops.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma, in linear light.
fn luma(px: &[f32]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// Clamped Hermite smoothstep. Same form as the one in `stages/vignette.rs`.
#[allow(dead_code)]
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

        let s = (self.params.detail / 100.0).clamp(0.0, 1.0);
        let k = (self.params.amount / 100.0).clamp(0.0, 1.0) * MAX_AMOUNT;

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .enumerate()
            .zip(image.data().par_chunks_exact(3))
            .for_each(|((i, o), px)| {
                let dv = d[i];
                let supp = s + (1.0 - s) / (1.0 + (dv.abs() / HALO_KNEE).powi(2));
                let delta = k * dv * supp;
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
        for (o, s) in out.data().chunks_exact(3).zip(src.data().chunks_exact(3)) {
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
