use crate::spatial::{guided_filter, min_filter};
use crate::{PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::Dehaze;
use rayon::prelude::*;

/// Haze deliberately retained at full strength. Removing all of it flattens aerial
/// perspective, and keeping a little also softens the bright-object failure case.
const OMEGA: f32 = 0.95;
/// Transmission floor: bounds the recovery gain to x10 in dense haze.
const T_MIN: f32 = 0.1;
/// Dark-channel patch radius, in **full-resolution** pixels.
const PATCH_RADIUS_FULL: f32 = 20.0;
/// Guided-filter radius for refining the transmission, in full-resolution pixels.
/// Deliberately much larger than the patch: the refinement's job is to restore the edges the
/// patch minimum smeared.
const REFINE_RADIUS_FULL: f32 = 60.0;
/// Guided-filter regularisation, against **linear** luma variance.
const REFINE_EPS: f32 = 1e-4;
/// Long edge of the fixed-size image the airlight is estimated on.
const AIRLIGHT_SAMPLE: usize = 256;
/// Fraction of the darkest-channel histogram treated as "most hazy".
const AIRLIGHT_FRACTION: f32 = 0.001;
/// Airlight patch radius as a divisor of the sample's long edge (256/32 = 8 px).
///
/// ⚠ A fraction of the **sample**, not derived from the input's size. Deriving it from the
/// input made a small image ask for a patch wider than itself, which turned the "haziest
/// local region" into "the whole frame".
const AIRLIGHT_PATCH_DIV: usize = 32;
/// Floor on each channel of the airlight. Load-bearing: `I_c / A_c` divides by it.
const A_MIN: f32 = 1e-3;

/// Rec.709 luma, in linear light. Same as the sharpening and local-contrast stages.
fn luma(px: &[f32]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// Convert a full-resolution radius to the resolution being rendered. Never 0, which would
/// silently disable the filter at small preview scales. Mirrors `noise_reduction`.
fn scaled_radius(full_px: f32, scale: RenderScale) -> usize {
    ((full_px * scale.get()).round().max(0.0) as usize).max(1)
}

/// Per-pixel minimum across the three channels — the dark channel before the patch min.
fn channel_min(image: &ImageBuf) -> Vec<f32> {
    image
        .data()
        .par_chunks_exact(3)
        .map(|px| px[0].min(px[1]).min(px[2]))
        .collect()
}

/// Estimate the atmospheric light: the mean colour of the haziest pixels.
///
/// ⚠ Computed on a **fixed-size** downscale, never at the render resolution. An order
/// statistic over the brightest pixels moves when you downscale, so estimating it at the
/// render scale would give the fit preview, the 1:1 view and the export three different
/// airlights — and the image would visibly shift on zoom. Pinning the sample size makes this
/// render-scale-independent by construction.
///
/// The **mean** of the top fraction rather than the single brightest pixel, so one specular
/// highlight or hot pixel cannot set the airlight for the whole frame.
pub fn airlight(image: &ImageBuf) -> [f32; 3] {
    let small = image.downscale_to(AIRLIGHT_SAMPLE);
    let (w, h) = (small.width(), small.height());
    if w == 0 || h == 0 {
        return [A_MIN; 3];
    }
    // A fixed fraction of the sample, so "haziest" means the same thing at any input size.
    let r = (w.max(h) / AIRLIGHT_PATCH_DIV).max(1);
    let dark = min_filter(&channel_min(&small), w, h, r);

    let mut order: Vec<usize> = (0..w * h).collect();
    order.sort_unstable_by(|&a, &b| dark[b].total_cmp(&dark[a]));
    let take = ((w * h) as f32 * AIRLIGHT_FRACTION).round().max(1.0) as usize;

    let mut sum = [0.0f32; 3];
    for &i in order.iter().take(take) {
        let px = small.pixel(i % w, i / w);
        for (s, v) in sum.iter_mut().zip(px) {
            *s += v;
        }
    }
    let n = take as f32;
    [
        (sum[0] / n).max(A_MIN),
        (sum[1] / n).max(A_MIN),
        (sum[2] / n).max(A_MIN),
    ]
}

/// The refined transmission map for `image`: how much of each pixel survived the haze.
pub fn transmission(image: &ImageBuf, a: [f32; 3], scale: RenderScale) -> Vec<f32> {
    let (w, h) = (image.width(), image.height());
    // min over channels of I_c / A_c, then min over the patch.
    let normalized: Vec<f32> = image
        .data()
        .par_chunks_exact(3)
        .map(|px| (px[0] / a[0]).min(px[1] / a[1]).min(px[2] / a[2]))
        .collect();
    let dark = min_filter(&normalized, w, h, scaled_radius(PATCH_RADIUS_FULL, scale));
    let coarse: Vec<f32> = dark.iter().map(|&d| 1.0 - OMEGA * d).collect();

    // Refine against image luma. Without this the map is blocky, and any plain blur of it
    // produces halos at depth discontinuities — sky against ridge, which is where dehaze is
    // used most.
    let guide: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();
    guided_filter(
        &coarse,
        &guide,
        w,
        h,
        scaled_radius(REFINE_RADIUS_FULL, scale),
        REFINE_EPS,
    )
}

/// Dehaze via the dark-channel prior, on the haze model `I = J·t + A·(1 − t)`.
///
/// The model is **radiometric**, so it holds exactly in this pipeline's linear working space
/// and only approximately in the gamma-encoded space most published implementations use.
///
/// Both slider directions share one expression. With `k = amount/100` and
/// `t_k = 1 − |k|·(1 − t)`: positive **inverts** the model to remove haze, negative **applies**
/// it to add haze. At `k = 0`, `t_k = 1` and both collapse to `J = I` exactly. Because `t`
/// encodes depth, adding haze is depth-aware for free.
pub struct DehazeStage {
    params: Dehaze,
    scale: RenderScale,
}

impl DehazeStage {
    pub fn new(params: &Dehaze, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for DehazeStage {
    fn name(&self) -> &'static str {
        "dehaze"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let a = airlight(&image);
        let t = transmission(&image, a, self.scale);
        let k = (self.params.amount / 100.0).clamp(-1.0, 1.0);
        let remove = k > 0.0;
        let mag = k.abs();

        let mut data = vec![0.0f32; w * h * 3];
        data.par_chunks_exact_mut(3)
            .zip(image.data().par_chunks_exact(3))
            .zip(&t)
            .for_each(|((o, px), &tv)| {
                let tk = 1.0 - mag * (1.0 - tv);
                for c in 0..3 {
                    let v = if remove {
                        (px[c] - a[c]) / tk.max(T_MIN) + a[c]
                    } else {
                        px[c] * tk + a[c] * (1.0 - tk)
                    };
                    // A pixel darker than the airlight can be driven negative by the
                    // division; clamp at 0, the convention every other stage uses.
                    o[c] = v.max(0.0);
                }
            });
        ImageBuf::from_vec(w, h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic scene: a colourful gradient with structure in all three channels.
    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                // ⚠ Periodic dark cells, on a 40 px pitch so a 41 px patch window always
                // contains one. The dark-channel prior *assumes* outdoor scenes have
                // near-black samples in every patch; a smooth gradient violates that, and a
                // fixture that violates the model measures how the model degrades rather
                // than whether we implemented it.
                let dark_cell = x % 40 < 6 && y % 40 < 6;
                let px = if dark_cell {
                    [0.01, 0.01, 0.01]
                } else {
                    [
                        0.05 + 0.7 * u,
                        0.05 + 0.6 * v,
                        0.05 + 0.5 * (1.0 - u) * (1.0 - v),
                    ]
                };
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    /// Transmission that falls off to the right: the left of the frame is near, the right far.
    ///
    /// ⚠ Reaches 0.05, not 0.3. The airlight can only be *observed* where transmission is
    /// near zero — a real hazy scene has a horizon that is essentially pure airlight, and
    /// that is what the estimate keys on. A fixture whose haze tops out at 70% gives the
    /// estimator nothing to find and caps how well recovery can possibly work.
    fn depth_t(x: usize, w: usize) -> f32 {
        0.95 - 0.9 * (x as f32 / w as f32)
    }

    /// Apply the haze model forward, producing a hazy image from a known scene.
    fn add_haze(clear: &ImageBuf, a: [f32; 3]) -> ImageBuf {
        let (w, h) = (clear.width(), clear.height());
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let px = clear.pixel(x, y);
                let t = depth_t(x, w);
                out.set_pixel(
                    x,
                    y,
                    [
                        px[0] * t + a[0] * (1.0 - t),
                        px[1] * t + a[1] * (1.0 - t),
                        px[2] * t + a[2] * (1.0 - t),
                    ],
                );
            }
        }
        out
    }

    fn run(amount: f32, img: ImageBuf) -> ImageBuf {
        DehazeStage::new(&Dehaze { amount }, RenderScale::FULL).apply(img)
    }

    /// Mean absolute per-channel difference over the interior.
    fn mean_diff(a: &ImageBuf, b: &ImageBuf, border: usize) -> f32 {
        let (mut sum, mut n) = (0.0f32, 0usize);
        for y in border..a.height() - border {
            for x in border..a.width() - border {
                for c in 0..3 {
                    sum += (a.pixel(x, y)[c] - b.pixel(x, y)[c]).abs();
                    n += 1;
                }
            }
        }
        sum / n as f32
    }

    #[test]
    fn amount_zero_is_an_exact_identity() {
        // Every existing golden depends on this.
        let src = scene(64, 48);
        assert_eq!(run(0.0, src.clone()), src);
    }

    #[test]
    fn synthetic_haze_is_removed() {
        // ⚠ The test that carries the weight: a genuine inverse-problem check against a known
        // answer, not "the output differs from the input".
        //
        // A known scene is hazed with a known airlight and depth, then dehazed. The result
        // must be much closer to the original than the hazy version was. Not exact — OMEGA
        // deliberately retains some haze and T_MIN clamps the far end — so this asserts a
        // large *improvement*, which is the honest claim.
        // ⚠ Large enough that REFINE_RADIUS_FULL (60) is a *local* window. On a 96 px
        // image the refinement covers the whole frame, globalises `t`, and destroys the very
        // depth structure this test is checking for.
        let clear = scene(384, 256);
        let a = [0.75f32, 0.78, 0.85];
        let hazy = add_haze(&clear, a);
        let recovered = run(100.0, hazy.clone());

        let before = mean_diff(&hazy, &clear, 8);
        let after = mean_diff(&recovered, &clear, 8);
        assert!(
            after < before * 0.5,
            "dehaze did not recover the scene: {before} -> {after}"
        );
    }

    #[test]
    fn a_clear_image_is_barely_changed() {
        // The dark channel of a scene with real shadows is near zero, so t ~ 1 and there is
        // almost nothing to remove.
        let clear = scene(384, 256);
        let out = run(100.0, clear.clone());
        let hazy = add_haze(&clear, [0.75, 0.78, 0.85]);
        assert!(
            mean_diff(&out, &clear, 8) < mean_diff(&hazy, &clear, 8) * 0.5,
            "a clear image was changed as much as a hazy one"
        );
    }

    #[test]
    fn negative_dehaze_is_depth_aware() {
        // ⚠ The payoff of reusing the transmission map for the negative direction.
        //
        // Asserted as the **retention ratio** |out - A| / |in - A|, which is exactly `t_k`.
        // The obvious alternative — how far each pixel *moved* — is |J - A|·t·(1 - t), which
        // is confounded by how far the scene colour happens to sit from the airlight and is
        // not even monotonic in t. An earlier version of this test measured that and
        // concluded the effect was backwards.
        //
        // A flat scene, so |J - A| is constant and only depth varies.
        let (w, h) = (384usize, 256usize);
        let a = [0.80f32, 0.82, 0.88];
        let mut flat = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                flat.set_pixel(x, y, [0.15, 0.18, 0.22]);
            }
        }
        let src = add_haze(&flat, a);
        let out = run(-100.0, src.clone());

        // ⚠ Normalised against the airlight the stage *estimated*, not the one the fixture
        // imposed. The stage moves pixels toward its own estimate, so measuring against the
        // true value conflates "how much haze was added" with "how far the estimate is off"
        // — which is what made an earlier version of this test read backwards.
        let a_hat = airlight(&src);
        let retention = |x: usize| -> f32 {
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for y in 40..h - 40 {
                num += (out.pixel(x, y)[1] - a_hat[1]).abs();
                den += (src.pixel(x, y)[1] - a_hat[1]).abs();
            }
            num / den
        };
        let near = retention(40);
        let far = retention(w - 41);
        assert!(
            far < near * 0.7,
            "haze was not depth-aware: retention near {near}, far {far}"
        );
    }

    #[test]
    fn the_airlight_is_independent_of_the_render_scale() {
        // ⚠ The fixed-256 px decision, asserted directly. Estimating at the render scale
        // would make the fit preview, the 1:1 view and the export disagree.
        let big = add_haze(&scene(512, 384), [0.8, 0.8, 0.9]);
        let small = big.downscale_to(200);
        let a_big = airlight(&big);
        let a_small = airlight(&small);
        for c in 0..3 {
            assert!(
                (a_big[c] - a_small[c]).abs() < 0.05,
                "channel {c}: {} vs {}",
                a_big[c],
                a_small[c]
            );
        }
    }

    #[test]
    fn the_airlight_ignores_a_single_hot_pixel() {
        // The mean-of-top-fraction choice: one blown speculum must not set A for the frame.
        let mut img = add_haze(&scene(128, 96), [0.7, 0.7, 0.75]);
        let clean = airlight(&img);
        img.set_pixel(64, 48, [40.0, 40.0, 40.0]);
        let with_spike = airlight(&img);
        for c in 0..3 {
            assert!(
                (with_spike[c] - clean[c]).abs() < 0.2,
                "channel {c} moved from {} to {}",
                clean[c],
                with_spike[c]
            );
        }
    }

    #[test]
    fn a_black_frame_does_not_divide_by_zero() {
        // The A_MIN floor. Without it, `I_c / A_c` is 0/0.
        let black = ImageBuf::new(32, 24);
        let a = airlight(&black);
        assert!(a.iter().all(|v| *v >= A_MIN));
        let out = run(100.0, black);
        assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        let hazy = add_haze(&scene(96, 64), [0.9, 0.9, 0.95]);
        for amount in [-100.0, -50.0, 50.0, 100.0] {
            let out = run(amount, hazy.clone());
            assert!(
                out.data().iter().all(|v| v.is_finite() && *v >= 0.0),
                "amount {amount}"
            );
        }
    }

    #[test]
    fn the_patch_radius_scales_with_the_render_scale() {
        assert_eq!(scaled_radius(PATCH_RADIUS_FULL, RenderScale::FULL), 20);
        assert_eq!(scaled_radius(PATCH_RADIUS_FULL, RenderScale::new(0.25)), 5);
        // Never 0, which would silently disable the patch minimum on a small preview.
        assert_eq!(scaled_radius(PATCH_RADIUS_FULL, RenderScale::new(0.001)), 1);
        // And it actually changes the pixels.
        let hazy = add_haze(&scene(96, 64), [0.8, 0.8, 0.85]);
        let full =
            DehazeStage::new(&Dehaze { amount: 80.0 }, RenderScale::FULL).apply(hazy.clone());
        let quarter =
            DehazeStage::new(&Dehaze { amount: 80.0 }, RenderScale::new(0.25)).apply(hazy);
        assert_ne!(full, quarter);
    }
}
