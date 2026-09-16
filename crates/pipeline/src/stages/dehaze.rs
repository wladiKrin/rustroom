use crate::spatial::{guided_filter, min_filter};
use crate::{PipelineStage, Rect, RenderScale};
use core_image::ImageBuf;
use edit_model::Dehaze;
use rayon::prelude::*;

/// Haze deliberately retained at full strength. Removing all of it flattens aerial
/// perspective, and keeping a little also softens the bright-object failure case.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const OMEGA: f32 = 0.95;
/// Transmission floor: bounds the recovery gain to x10 in dense haze.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const T_MIN: f32 = 0.1;
/// Dark-channel patch radius, in **full-resolution** pixels.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const PATCH_RADIUS_FULL: f32 = 20.0;
/// Guided-filter radius for refining the transmission, in full-resolution pixels.
/// Deliberately much larger than the patch: the refinement's job is to restore the edges the
/// patch minimum smeared.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const REFINE_RADIUS_FULL: f32 = 60.0;
/// Guided-filter regularisation, against **linear** luma variance.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const REFINE_EPS: f32 = 1e-4;
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
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const A_MIN: f32 = 1e-3;

/// Distance from the airlight, in **linear working-space** units and as the largest per-channel
/// difference, within which a pixel is read as an object of that colour rather than as haze.
///
/// ⚠⚠ **Measured, not reasoned** (`dehaze_bright_probe`, 415 X-T4 frames, 2026-09-08). The
/// dark-channel prior reads a white wall as dense haze — its dark channel is high — and pushes
/// every wall pixel darker than the airlight estimate further from it, which at +100 darkened
/// the two white-kitten frames **15.7–18.7%** on average and 38–48% in their worst decile, with
/// a blue-grey tint (Oklab chroma +0.013). Swept at +100:
///
/// | K | white wall (DSCF6546 / 6557) | synthetic haze residual | depth-aware test |
/// | --- | --- | --- | --- |
/// | 0 | 18.7% / 15.7% darker | 0.336 | pass |
/// | 0.06 | 10.4% / 3.8% | — | pass |
/// | **0.1** | **3.4% / 0.8%** | **0.380** | pass |
/// | 0.2 | 0.0% / 0.0% | — | **FAILS** — the depth gradient is swallowed |
///
/// 0.1 is the smallest value that removes the white-wall case, and 0.2 is past the ceiling.
/// The cost is ~13% less removal of dense haze (0.336 → 0.380 of the haze left), because dense
/// haze is near the airlight by definition; `synthetic_haze_is_removed`'s bound of 0.5 is what
/// keeps that honest. Cloudy skies (~15% at +100) are *not* near the airlight and are untouched —
/// deliberately, since the +60 renders show the sky effect people reach for Dehaze to get.
///
/// ⚠ A fixed constant, no slider: Lightroom exposes none, and the Clarity halo suppressor set the
/// precedent that this is part of what the tool means.
/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it. A
/// substituted constant **cannot** disagree; a transcribed one is merely checked for agreeing.
pub const AIRLIGHT_TOLERANCE: f32 = 0.1;

/// The tolerance rule: `t` raised toward 1 as `dist` falls below `tolerance`.
///
/// Continuous: 1 for `dist <= tolerance·t`, `t·tolerance/dist` between there and `tolerance`
/// (reaching exactly `t`), and `t` beyond. Only the derivative kinks, and the guided refinement
/// this runs ahead of smooths that spatially, as it already smooths the patch minimum's blocks.
/// A zero tolerance is the identity.
fn tolerant_transmission(t: f32, dist: f32, tolerance: f32) -> f32 {
    if tolerance <= 0.0 {
        return t;
    }
    (t * (tolerance / dist.max(1e-6)).max(1.0)).min(1.0)
}

/// Rec.709 luma, in linear light. Same as the sharpening and local-contrast stages.
fn luma(px: &[f32]) -> f32 {
    // ⚠ Delegates to `color::luminance`, which carries the **working space's** weights. This
    // used to be a local copy of the Rec.709 triple — one of six, all of which became wrong the
    // day the working space stopped being sRGB.
    color::luminance([px[0], px[1], px[2]])
}

/// Convert a full-resolution radius to the resolution being rendered. Never 0, which would
/// silently disable the filter at small preview scales. Mirrors `noise_reduction`.
pub fn scaled_radius(full_px: f32, scale: RenderScale) -> usize {
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

/// The refined transmission map for `image`: how much of each pixel survived the haze, with
/// the airlight tolerance passed in.
///
/// ⚠ The tolerance is a parameter rather than read from the constant here so the tests can
/// compare against the untreated stage (a tolerance of 0). Production reaches this only through
/// `DehazeStage::apply`, which passes [`AIRLIGHT_TOLERANCE`]. ⚠ A parameterless wrapper was
/// tried and was dead code: `stages` is a private module, so a `pub fn` with no caller fails
/// `clippy -D warnings` — the trap this repo records for every such helper.
pub(crate) fn transmission_with(
    image: &ImageBuf,
    a: [f32; 3],
    scale: RenderScale,
    tolerance: f32,
) -> Vec<f32> {
    let (w, h) = (image.width(), image.height());
    // min over channels of I_c / A_c, then min over the patch.
    let normalized: Vec<f32> = image
        .data()
        .par_chunks_exact(3)
        .map(|px| (px[0] / a[0]).min(px[1] / a[1]).min(px[2] / a[2]))
        .collect();
    let dark = min_filter(&normalized, w, h, scaled_radius(PATCH_RADIUS_FULL, scale));
    // ⚠ The tolerance is applied to the COARSE map, before the refinement, so the kink at its
    // breakpoints is smoothed along with everything else. Largest per-channel distance: a pixel
    // is "the airlight's colour" only when every channel is within the tolerance.
    let dist: Vec<f32> = image
        .data()
        .par_chunks_exact(3)
        .map(|px| {
            (px[0] - a[0])
                .abs()
                .max((px[1] - a[1]).abs())
                .max((px[2] - a[2]).abs())
        })
        .collect();
    let coarse: Vec<f32> = dark
        .iter()
        .zip(&dist)
        .map(|(&d, &ds)| tolerant_transmission(1.0 - OMEGA * d, ds, tolerance))
        .collect();

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

/// Both slider directions share one expression. With `k = amount/100` and
/// `t_k = 1 − |k|·(1 − t)`: positive **inverts** the model to remove haze, negative **applies**
/// it to add haze. At `k = 0`, `t_k = 1` and both collapse to `J = I` exactly. Because `t`
/// encodes depth, adding haze is depth-aware for free.
/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// ⚠⚠ The two terms **add**, unlike `noise_reduction::kernel_reach`'s max. The dark channel is a
/// `min_filter` of radius [`PATCH_RADIUS_FULL`] producing a transmission map; the guided
/// refinement then reads a `2 * REFINE_RADIUS_FULL` neighbourhood **of that map**. So the
/// dependence composes: 120 + 20 = 140 full-resolution pixels.
///
/// ⚠ `2 *` because a guided base reaches `2r` — every window statistic is a box blur of radius
/// `r` and the coefficients are blurred again. `local_contrast::kernel_reach`'s rule.
///
/// ⚠⚠ **The airlight contributes nothing here, and that is the whole point of making it a
/// parameter** — see [`crate::prefix_airlight`]. Were it still estimated from the stage's own
/// input, this stage's true reach would be the **entire frame** and no margin could ever be
/// correct.
///
/// Any renderer that crops a region before running this stage must keep at least this much margin
/// outside what it displays — see [`crate::region_plan`].
pub fn kernel_reach(params: &Dehaze, scale: RenderScale) -> usize {
    if !params.is_active() {
        return 0;
    }
    2 * scaled_radius(REFINE_RADIUS_FULL, scale) + scaled_radius(PATCH_RADIUS_FULL, scale)
}

pub struct DehazeStage {
    params: Dehaze,
    scale: RenderScale,
    /// The frame-global airlight, when the caller supplies one.
    ///
    /// ⚠⚠ **`None` means "estimate from the image I am handed", which is what shipped before
    /// M8.7 and is still correct for a whole-buffer render.** It is *not* correct under a
    /// region-scoped run, where the image in hand is a slice and the estimate would change as you
    /// pan — the failure [`crate::PipelineStage::apply_region`]'s `None` default exists to prevent.
    airlight: Option<[f32; 3]>,
}

impl DehazeStage {
    pub fn new(params: &Dehaze, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
            airlight: None,
        }
    }

    /// Build with the airlight supplied rather than estimated from this stage's input.
    ///
    /// ⚠⚠ **This is what lets dehaze be region-scoped at all.** Estimated from the input, the
    /// stage's true reach is the ENTIRE FRAME and no margin could ever be correct — see
    /// [`kernel_reach`], which counts only the two spatial terms precisely because the airlight is
    /// no longer one of them.
    ///
    /// ⚠ Supplied by [`crate::prefix_airlight`], which replays the prefix at a fixed 256 px so the
    /// value is a deterministic function of the edits alone.
    pub fn with_airlight(params: &Dehaze, scale: RenderScale, airlight: [f32; 3]) -> Self {
        Self {
            params: *params,
            scale,
            airlight: Some(airlight),
        }
    }
}

impl PipelineStage for DehazeStage {
    fn name(&self) -> &'static str {
        "dehaze"
    }

    /// ⚠ Grows by [`kernel_reach`] on every side. Dimension-preserving, like noise reduction, so
    /// `out` and the result are in the same space.
    fn input_rect_for(&self, out: crate::Rect, in_w: usize, in_h: usize) -> Option<crate::Rect> {
        Some(out.grown(kernel_reach(&self.params, self.scale), in_w, in_h))
    }

    /// Produce only `out` of this stage's output, from a margined region of its input.
    ///
    /// ⚠⚠ **Declines unless the airlight was supplied**, and that is the whole safety argument.
    /// Without one this stage would estimate from the slice in hand, so the photograph would change
    /// as you pan — and it would do so plausibly rather than visibly. Failing toward the
    /// whole-buffer path is `GpuStage::cpu`'s rule and `apply_region`'s own.
    ///
    /// ⚠ Given one, the result is exact for every pixel in `out`: [`crate::region_plan`] grows the
    /// request by [`kernel_reach`] on every side, so a pixel inside `out` reads only samples that
    /// are inside the buffer, and those hold what the whole image holds there.
    fn apply_region(&self, src: &crate::RegionSrc<'_>, out: Rect) -> Option<ImageBuf> {
        self.airlight?;
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

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        self.apply_with(image, AIRLIGHT_TOLERANCE)
    }
}

impl DehazeStage {
    /// [`PipelineStage::apply`] with the airlight tolerance overridden — the tests' seam, the
    /// `local_contrast::apply_with` precedent. Production only ever calls `apply`.
    pub(crate) fn apply_with(&self, image: ImageBuf, tolerance: f32) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let a = self.airlight.unwrap_or_else(|| airlight(&image));
        let t = transmission_with(&image, a, self.scale, tolerance);
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
    #[test]
    fn the_kernel_reach_is_the_refinement_plus_the_dark_channel_patch() {
        // ⚠⚠ The two genuinely ADD, unlike noise reduction's two halves. The dark channel is a
        // min_filter of radius 20 producing a transmission map; the guided refinement then reads a
        // 2 x 60 neighbourhood OF THAT map. So one output pixel depends on the source within
        // 120 + 20 = 140.
        let p = super::Dehaze { amount: 60.0 };
        assert_eq!(super::kernel_reach(&p, super::RenderScale::FULL), 140);
    }

    #[test]
    fn an_inactive_dehaze_reaches_nothing() {
        assert_eq!(
            super::kernel_reach(&super::Dehaze { amount: 0.0 }, super::RenderScale::FULL),
            0
        );
    }

    #[test]
    fn the_dehaze_reach_follows_the_render_scale() {
        // ⚠ At RenderScale::FULL the scaling is the identity, so the tests above cannot see it
        // being dropped. This is the only one that can.
        let p = super::Dehaze { amount: 60.0 };
        assert_eq!(
            super::kernel_reach(&p, super::RenderScale::new(0.5)),
            2 * 30 + 10
        );
    }
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

    /// ⚠⚠ **The test this stage did not have**, for the same reason noise reduction did not:
    /// while `apply_region` computed the whole buffer and cropped, this compared a computation
    /// with itself and passed for that reason.
    ///
    /// ⚠ To within a rounding, NOT byte-for-byte — `a_chain_with_a_local_agrees_to_within_a_
    /// rounding` records why, and the same bound is used here.
    #[test]
    fn a_region_matches_the_same_part_of_the_whole() {
        let params = Dehaze { amount: 60.0 };
        let reach = kernel_reach(&params, RenderScale::FULL);
        assert!(reach > 0, "a zero reach would make the margin below vacuous");

        let side = 4 * reach + 64;
        let clear = scene(side, side);
        let a = [0.72, 0.76, 0.80];
        let hazy = add_haze(&clear, a);
        // ⚠ The airlight is SUPPLIED, exactly as `prefix_airlight` supplies it in the engine.
        // Estimated per-call it would differ between the whole buffer and a slice, and this would
        // measure that rather than the margin.
        let stage = DehazeStage::with_airlight(&params, RenderScale::FULL, a);

        let whole = stage.apply(hazy.clone());
        // Fixture check: a stage that moved nothing would make the comparison vacuous.
        assert_ne!(whole.data(), hazy.data(), "the stage must move pixels");

        // ⚠⚠ INTERIOR by more than `reach` on every side — an edge rect would pass with the
        // margin deleted, because the whole-buffer path clamps its window there too.
        let rect = Rect {
            x: 2 * reach,
            y: 2 * reach,
            w: 64,
            h: 64,
        };
        let got = stage
            .apply_region(&crate::RegionSrc::whole(&hazy), rect)
            .expect("a region inside the buffer, with an airlight, must be produced");
        let want = whole.crop(rect.x, rect.y, rect.w, rect.h);
        assert_eq!(got.width(), rect.w);
        assert_eq!(got.height(), rect.h);
        let worst = got
            .data()
            .iter()
            .zip(want.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1e-5,
            "a running sum may wobble in the last bits; a margin bug may not. worst {worst:e}"
        );
    }

    fn run_with_tolerance(amount: f32, img: ImageBuf, tolerance: f32) -> ImageBuf {
        DehazeStage::new(&Dehaze { amount }, RenderScale::FULL).apply_with(img, tolerance)
    }

    /// Mean luma over a rectangle.
    fn mean_luma(img: &ImageBuf, x0: usize, x1: usize, y0: usize, y1: usize) -> f32 {
        let mut sum = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += luma(&img.pixel(x, y));
            }
        }
        sum / ((x1 - x0) * (y1 - y0)) as f32
    }

    /// ⚠⚠ The artifact as a test, shaped like the frames that showed it: a large white wall
    /// with a 3% brightness falloff (flash, or a window at one side) inside a hazy scene. The
    /// wall's brightest rows out-brighten the haze's dark channel and **become the airlight
    /// estimate** — exactly as on the real white-wall frames — so every wall pixel below them
    /// reads as haze and is pushed away from the estimate. The band ~1% below the top is
    /// measured: **0.3% darker with the tolerance, 9.0% without** (measured, 2026-09-08).
    ///
    /// ⚠ Three things about the fixture were each found by a failing assertion, not designed:
    /// (1) a **flat** patch at 0.99 × A becomes the airlight itself and is untouched with or
    /// without the tolerance (`(Â − Â)/t + Â = Â`) — the falloff is what puts pixels *below* the
    /// estimate, which is the whole mechanism; (2) hazed with the **same** airlight colour, the
    /// far hazy corner ties with the wall top for the estimate, which then sits above the band
    /// in some channels and below it in others — so the scene is hazed 10% darker than the wall;
    /// (3) the guided refinement's reach is **twice** its radius (120 px at full scale, the
    /// `local_contrast::kernel_reach` finding), so a band 60 px below the wall's top edge was
    /// still averaged with the hazy rows above it and read a transmission of 0.17 rather than
    /// ~0.06. The wall therefore spans the **full image height** — no top edge inside the frame —
    /// and the band is inset 120 px from the wall's sides. ⚠ Even so the refinement averages the
    /// band with rows whose transmission is higher, so the untreated darkening is 9%, not the
    /// ~15% the per-pixel arithmetic predicts; the premise bound is 5%, a measured 4.5× margin
    /// over the treated case rather than a number reasoned from the model.
    ///
    /// Sabotage: `AIRLIGHT_TOLERANCE = 0.0` fails the first assertion.
    #[test]
    fn a_bright_object_near_the_airlight_is_left_alone() {
        let a = [0.75f32, 0.78, 0.85];
        let (w, h) = (384usize, 256usize);
        let mut hazy = add_haze(&scene(w, h), [0.9 * a[0], 0.9 * a[1], 0.9 * a[2]]);
        // The wall: x 40..340 over the whole height, 1.00 × A at the top row falling to 0.97 × A.
        let (px0, px1) = (40usize, 340usize);
        for y in 0..h {
            let f = 1.0 - 0.03 * y as f32 / (h - 1) as f32;
            for x in px0..px1 {
                hazy.set_pixel(x, y, [f * a[0], f * a[1], f * a[2]]);
            }
        }
        // The band where f is 0.989..0.991 — about 1% below the top — 120 px in from each side.
        let (ix0, ix1, iy0, iy1) = (px0 + 120, px1 - 120, 77usize, 93usize);
        let before = mean_luma(&hazy, ix0, ix1, iy0, iy1);

        let with = run(100.0, hazy.clone());
        let darkening = 1.0 - mean_luma(&with, ix0, ix1, iy0, iy1) / before;
        assert!(
            darkening < 0.02,
            "the wall near the airlight was darkened {:.1}%",
            darkening * 100.0
        );

        // The premise, asserted rather than assumed: without the tolerance the same band IS
        // read as haze and darkened well past what the first assertion allows.
        let without = run_with_tolerance(100.0, hazy, 0.0);
        let untreated = 1.0 - mean_luma(&without, ix0, ix1, iy0, iy1) / before;
        assert!(
            untreated > 0.05,
            "premise: without the tolerance the wall darkened only {:.1}%",
            untreated * 100.0
        );
    }

    /// The rule is continuous in the distance from the airlight: 1 close to it, the raw
    /// transmission far from it, and no jump at either breakpoint (`K·t` and `K`). Sabotage: a
    /// hard `if dist < K { 1.0 } else { t }` fails it at `dist = K`.
    #[test]
    fn the_tolerance_is_continuous() {
        let (t, k) = (0.3f32, AIRLIGHT_TOLERANCE);
        let n = 2000;
        let mut prev = tolerant_transmission(t, 0.0, k);
        assert!(
            (prev - 1.0).abs() < 1e-6,
            "at the airlight itself t must be 1"
        );
        for i in 1..=n {
            let dist = 2.0 * k * i as f32 / n as f32;
            let cur = tolerant_transmission(t, dist, k);
            assert!(
                (cur - prev).abs() < 0.01,
                "jump of {} at dist {dist}",
                (cur - prev).abs()
            );
            prev = cur;
        }
        assert!(
            (prev - t).abs() < 1e-6,
            "far from the airlight t must be untouched"
        );
        // And a zero tolerance is the identity, whatever the distance.
        assert_eq!(tolerant_transmission(t, 0.001, 0.0), t);
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
