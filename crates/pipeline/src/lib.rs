//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

pub mod spatial;
mod stages;

pub use stages::contrast::ContrastStage;
pub use stages::exposure::ExposureStage;
pub use stages::geometry::GeometryStage;
pub use stages::hsl::HslStage;
pub use stages::local_contrast::LocalContrastStage;
pub use stages::noise_reduction::NoiseReductionStage;
pub use stages::output_transform::OutputTransformSrgb;
pub use stages::saturation::SaturationStage;
pub use stages::sharpening::{max_kernel_radius, SharpenStage, RADIUS_RANGE};
pub use stages::tone_curve::ToneCurveStage;
pub use stages::tone_regions::ToneRegionsStage;
pub use stages::vignette::VignetteStage;
pub use stages::white_balance::WhiteBalanceStage;

/// Ratio of the image being rendered to the full-resolution original: `1.0` at full res,
/// ≈0.27 for the 1600 px preview of a 6000 px file.
///
/// Spatial stages multiply their pixel radii by this, so a preview approximates the
/// export instead of contradicting it. Per-pixel stages ignore it entirely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderScale(f32);

impl RenderScale {
    /// Rendering at the original resolution.
    pub const FULL: RenderScale = RenderScale(1.0);

    /// Clamped to `(0, 1]`. Anything outside that (or non-finite) is a caller bug, but a
    /// clamped radius beats panicking on the render worker thread.
    pub fn new(scale: f32) -> Self {
        if scale.is_finite() {
            RenderScale(scale.clamp(1e-3, 1.0))
        } else {
            RenderScale::FULL
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// One stage of the develop pipeline. Pure: no shared state, independently testable.
/// (A future GPU stage implements the same trait — the seam for the Phase 5 port.)
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, image: ImageBuf) -> ImageBuf;
}

/// An ordered pipeline of stages.
pub struct Pipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl Pipeline {
    pub fn new(stages: Vec<Box<dyn PipelineStage>>) -> Self {
        Self { stages }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Stage names in run order. Useful when debugging a render, and it lets tests assert
    /// pipeline *order* rather than just its length.
    pub fn stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// Consume the pipeline, yielding its stages — lets `build_pipeline` concatenate the two
    /// halves of [`build_pipeline_split`].
    pub fn into_stages(self) -> Vec<Box<dyn PipelineStage>> {
        self.stages
    }

    /// Run all stages in order.
    pub fn run(&self, mut image: ImageBuf) -> ImageBuf {
        for stage in &self.stages {
            image = stage.apply(image);
        }
        image
    }
}

/// Where a cropped render sits inside the full frame.
///
/// `None` at a call site means "this buffer *is* the full frame" — what every existing caller
/// implicitly passes, and what position-dependent stages already assume. Only stages whose
/// output depends on absolute position need it; today that is the vignette alone, but any
/// future graduated or radial filter will want the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub full_w: usize,
    pub full_h: usize,
    pub x0: usize,
    pub y0: usize,
}

/// Everything [`build_pipeline_split`]'s prefix depends on, for caching its output.
///
/// **If you add a stage to the prefix, add its parameters here.** The tests in this module
/// assert that every other edit field leaves this key unchanged, so they will fail until you
/// do — which is the point, because a prefix stage missing from the key means the preview
/// cache silently serves stale pixels.
///
/// Compared with `==` rather than hashed, so there is no collision risk. The `f32` is safe in
/// a `PartialEq`: the only pathological value is `NaN`, which compares unequal to itself and
/// so fails toward a cache *miss*.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefixKey {
    geometry: edit_model::Geometry,
    noise_reduction: edit_model::NoiseReduction,
    /// Included because it sets the noise-reduction radii.
    scale: f32,
}

impl PrefixKey {
    pub fn new(edits: &Edits, scale: RenderScale) -> Self {
        Self {
            geometry: edits.geometry,
            noise_reduction: edits.noise_reduction,
            scale: scale.get(),
        }
    }
}

/// The develop pipeline in two halves, split where the preview cache checkpoints.
///
/// The **prefix** — geometry, then noise reduction — depends only on `edits.geometry` and
/// `edits.noise_reduction`, which is exactly what [`PrefixKey`] captures. Its output can
/// therefore be reused across every adjustment downstream, which is what makes editing with
/// noise reduction enabled affordable.
///
/// The **tail** is everything else and always ends in the output transform.
///
/// [`build_pipeline`] is defined as `prefix ++ tail`, so the two constructions cannot drift
/// apart. Add a stage here, not there.
pub fn build_pipeline_split_in_region(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
) -> (Pipeline, Pipeline) {
    let mut prefix: Vec<Box<dyn PipelineStage>> = Vec::new();

    // Geometry runs first, so all adjustments operate on the final cropped pixels.
    if edits.geometry != edit_model::Geometry::default() {
        prefix.push(Box::new(GeometryStage::new(&edits.geometry)));
    }
    // Noise reduction comes before every adjustment: denoising the source is far easier
    // than denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are
    // modifiers and must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        prefix.push(Box::new(NoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
        )));
    }

    let mut tail: Vec<Box<dyn PipelineStage>> = Vec::new();
    if edits.white_balance != edit_model::WhiteBalance::default() {
        tail.push(Box::new(WhiteBalanceStage::new(&edits.white_balance)));
    }
    if edits.exposure != 0.0 {
        tail.push(Box::new(ExposureStage::new(edits.exposure)));
    }
    if edits.contrast != 0.0 {
        tail.push(Box::new(ContrastStage::new(edits.contrast)));
    }
    if edits.highlights != 0.0 || edits.shadows != 0.0 || edits.whites != 0.0 || edits.blacks != 0.0
    {
        tail.push(Box::new(ToneRegionsStage::new(
            edits.highlights,
            edits.shadows,
            edits.whites,
            edits.blacks,
        )));
    }
    if !edits.tone_curve.points.is_empty() {
        tail.push(Box::new(ToneCurveStage::new(&edits.tone_curve.points)));
    }

    // Local contrast, in two bands. After the tone curve is mandatory: a local contrast
    // measured before it would then be re-mapped by it. Before the colour stages so they
    // see final tonality — numerically near-irrelevant, since the gain is scalar and
    // preserves r:g:b ratios, but the order should say what we mean.
    if edits.local_contrast.is_active() {
        tail.push(Box::new(LocalContrastStage::new(
            &edits.local_contrast,
            scale,
        )));
    }

    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        tail.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }

    if edits.hsl != edit_model::Hsl::default() {
        tail.push(Box::new(HslStage::new(&edits.hsl)));
    }

    // A creative effect over the finished grade, so it runs last. It is post-crop for free:
    // `GeometryStage` ran first, so the frame it sees is already cropped. It takes no
    // `RenderScale` — normalized coordinates make it resolution-independent.
    if edits.vignette.is_active() {
        tail.push(Box::new(VignetteStage::new_in_region(
            &edits.vignette,
            region,
        )));
    }

    // The last spatial stage, and the only one in the tail. It therefore sets the margin a
    // region render must keep outside the visible rect — see `max_kernel_radius`. Placed
    // after the vignette so a darkened corner sharpens proportionally less in absolute
    // terms, which is what a ratio-based model should do.
    if edits.sharpening.is_active() {
        tail.push(Box::new(SharpenStage::new(&edits.sharpening, scale)));
    }

    tail.push(Box::new(OutputTransformSrgb));

    (Pipeline::new(prefix), Pipeline::new(tail))
}

/// The develop pipeline in two halves, for a render covering the whole frame.
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region(edits, scale, None)
}

/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline {
    let (prefix, tail) = build_pipeline_split(edits, scale);
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    Pipeline::new(stages)
}

/// The widest neighbourhood any **tail** stage reads, in rendered pixels, for these edits.
///
/// The tail is the half that runs on a cropped region during a 1:1 render, so at the
/// region's edge a spatial tail stage reads neighbours that do not exist. This is the width
/// of that wrong band, and therefore the margin such a render must keep outside what it
/// displays. Prefix stages are exempt: the prefix always runs whole-buffer, which is why
/// noise reduction can never produce a seam however you pan.
///
/// An inactive stage contributes 0.
pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize {
    let sharp = if edits.sharpening.is_active() {
        stages::sharpening::max_kernel_radius(edits.sharpening.radius, scale)
    } else {
        0
    };
    let local = stages::local_contrast::kernel_reach(&edits.local_contrast, scale);
    sharp.max(local)
}

/// [`tail_kernel_radius`] maximised over every representable setting — what a fixed render
/// margin has to exceed.
///
/// Defined *through* `tail_kernel_radius` rather than duplicating the maxima, so the two
/// cannot drift apart when a stage is added.
pub fn max_tail_kernel_radius(scale: RenderScale) -> usize {
    let worst = Edits {
        sharpening: edit_model::Sharpening {
            amount: 100.0,
            radius: RADIUS_RANGE.1,
            ..Default::default()
        },
        local_contrast: edit_model::LocalContrast {
            texture: 100.0,
            clarity: 100.0,
        },
        ..Edits::default()
    };
    tail_kernel_radius(&worst, scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

    /// An `Edits` with every field non-neutral — so a stage that has drifted into the wrong
    /// half of the split shows up.
    ///
    /// Every field is listed explicitly rather than using `..Default::default()`, on purpose:
    /// adding a field to `Edits` then breaks this function's compilation, forcing whoever
    /// adds it to decide which half of the split it belongs to.
    fn all_edits() -> Edits {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            white_balance: edit_model::WhiteBalance {
                temp: 10.0,
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            vignette: edit_model::Vignette {
                amount: -30.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 60.0,
                masking: 40.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            version: 0,
        }
    }

    #[test]
    fn prefix_key_ignores_every_downstream_field() {
        // Proves every edit field is either in the key or downstream of the checkpoint.
        // If a field is added to the prefix without being added to PrefixKey, the cache
        // would silently serve stale pixels; this is what stands in the way.
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL);

        let mut hsl = edit_model::Hsl::default();
        hsl.bands[3].hue = 40.0;
        let downstream = [
            Edits {
                white_balance: edit_model::WhiteBalance {
                    temp: 50.0,
                    tint: -20.0,
                },
                ..base.clone()
            },
            Edits {
                exposure: 2.0,
                ..base.clone()
            },
            Edits {
                contrast: 50.0,
                ..base.clone()
            },
            Edits {
                highlights: -50.0,
                ..base.clone()
            },
            Edits {
                shadows: 50.0,
                ..base.clone()
            },
            Edits {
                whites: 30.0,
                ..base.clone()
            },
            Edits {
                blacks: -30.0,
                ..base.clone()
            },
            Edits {
                tone_curve: edit_model::ToneCurve {
                    points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
                },
                ..base.clone()
            },
            Edits {
                saturation: 40.0,
                ..base.clone()
            },
            Edits {
                vibrance: 40.0,
                ..base.clone()
            },
            Edits {
                hsl,
                ..base.clone()
            },
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 60.0,
                    clarity: -40.0,
                },
                ..base.clone()
            },
            Edits {
                vignette: edit_model::Vignette {
                    amount: -70.0,
                    ..Default::default()
                },
                ..base.clone()
            },
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 80.0,
                    ..Default::default()
                },
                ..base.clone()
            },
        ];
        for e in downstream {
            assert_eq!(
                PrefixKey::new(&e, RenderScale::FULL),
                k,
                "a downstream field changed the prefix key"
            );
        }
    }

    #[test]
    fn prefix_key_changes_with_prefix_fields_and_scale() {
        let base = Edits::default();
        let k = PrefixKey::new(&base, RenderScale::FULL);

        let geo = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(PrefixKey::new(&geo, RenderScale::FULL), k);

        let nr = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
            ..base.clone()
        };
        assert_ne!(PrefixKey::new(&nr, RenderScale::FULL), k);

        // Scale changes the noise-reduction radii, so it belongs in the key.
        assert_ne!(PrefixKey::new(&base, RenderScale::new(0.25)), k);
    }

    #[test]
    fn a_none_region_split_is_identical_to_the_plain_split() {
        for edits in [Edits::default(), all_edits()] {
            let (p1, t1) = build_pipeline_split(&edits, RenderScale::FULL);
            let (p2, t2) = build_pipeline_split_in_region(&edits, RenderScale::FULL, None);
            assert_eq!(p1.stage_names(), p2.stage_names());
            assert_eq!(t1.stage_names(), t2.stage_names());
        }
    }

    #[test]
    fn build_pipeline_is_exactly_prefix_then_tail() {
        for edits in [Edits::default(), all_edits()] {
            let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
            let mut expected = prefix.stage_names();
            expected.extend(tail.stage_names());
            assert_eq!(
                build_pipeline(&edits, RenderScale::FULL).stage_names(),
                expected
            );
        }
    }

    #[test]
    fn the_prefix_holds_only_geometry_and_noise_reduction() {
        // The tripwire: if a stage is later inserted into the prefix region, this fails and
        // forces PrefixKey to be updated with its parameters.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert_eq!(prefix.stage_names(), vec!["geometry", "noise_reduction"]);
        for name in tail.stage_names() {
            assert!(
                name != "geometry" && name != "noise_reduction",
                "prefix stage {name} leaked into the tail"
            );
        }
        assert_eq!(tail.stage_names().last(), Some(&"output_transform_srgb"));
    }

    #[test]
    fn running_prefix_then_tail_matches_the_whole_pipeline() {
        // The numerical counterpart to the name-based test above: bit-identical, not close.
        let edits = all_edits();
        let mut img = ImageBuf::new(16, 12);
        for y in 0..12 {
            for x in 0..16 {
                img.set_pixel(x, y, [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
            }
        }
        let whole = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
        let split = tail.run(prefix.run(img));
        assert_eq!(whole.data(), split.data());
    }

    #[test]
    fn render_scale_full_is_one() {
        assert_eq!(RenderScale::FULL.get(), 1.0);
    }

    #[test]
    fn render_scale_clamps_nonsense_input() {
        // A scale above 1 or at/below 0 is a caller bug, but the render worker must not
        // panic over it — a clamped radius is a recoverable wrong answer.
        assert_eq!(RenderScale::new(4.0).get(), 1.0);
        assert_eq!(RenderScale::new(0.0).get(), RenderScale::new(-1.0).get());
        assert!(RenderScale::new(0.0).get() > 0.0);
        assert_eq!(RenderScale::new(f32::NAN).get(), 1.0);
        assert_eq!(RenderScale::new(0.25).get(), 0.25);
    }

    #[test]
    fn per_pixel_stages_ignore_the_render_scale() {
        // Everything that existed before noise reduction is per-pixel, so scale must be
        // a no-op for it — bit for bit, not just visually.
        let edits = Edits {
            exposure: 0.7,
            contrast: 25.0,
            saturation: 30.0,
            ..Edits::default()
        };
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.1 * x as f32, 0.2, 0.05 * y as f32]);
            }
        }
        let full = build_pipeline(&edits, RenderScale::FULL).run(img.clone());
        let quarter = build_pipeline(&edits, RenderScale::new(0.25)).run(img);
        assert_eq!(full.data(), quarter.data());
    }

    #[test]
    fn stage_names_are_reported_in_order() {
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["exposure", "saturation", "output_transform_srgb"]
        );
    }

    #[test]
    fn output_transform_encodes_linear_to_srgb() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.5, 0.5, 0.5]);
        let out = build_pipeline(&Edits::default(), RenderScale::FULL).run(img);
        let [r, g, b] = out.pixel(0, 0);
        let expected = color::linear_to_srgb(0.5);
        assert!((r - expected).abs() < 1e-4);
        assert!((g - expected).abs() < 1e-4);
        assert!((b - expected).abs() < 1e-4);
    }

    #[test]
    fn default_pipeline_has_output_transform_last() {
        let p = build_pipeline(&Edits::default(), RenderScale::FULL);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn nonneutral_edits_add_stages_before_output_transform() {
        let edits = Edits {
            exposure: 1.0,
            contrast: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 3);
    }

    #[test]
    fn hsl_adds_one_stage_when_non_neutral() {
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[2].saturation = 30.0;
        let edits = Edits {
            hsl,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn neutral_hsl_adds_no_stage() {
        let edits = Edits {
            hsl: edit_model::Hsl::default(),
            saturation: 10.0,
            ..Edits::default()
        };
        // saturation stage + output transform only
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn saturation_adds_one_stage() {
        let edits = Edits {
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn vibrance_alone_adds_one_stage() {
        let edits = Edits {
            vibrance: -30.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn neutral_color_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn noise_reduction_runs_before_every_adjustment() {
        // Early placement is the whole point: denoise the source rather than fight noise
        // that a Shadows lift has already multiplied up. Geometry stays first so NR runs
        // on the cropped pixels.
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                color: 40.0,
                ..Default::default()
            },
            white_balance: edit_model::WhiteBalance {
                temp: 20.0,
                ..Default::default()
            },
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "geometry",
                "noise_reduction",
                "white_balance",
                "exposure",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn noise_reduction_modifiers_alone_add_no_stage() {
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 0.0,
                detail: 100.0,
                smoothness: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn vignette_runs_last_before_the_output_transform() {
        // A creative effect laid over the finished grade.
        let edits = Edits {
            exposure: 1.0,
            saturation: 20.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "saturation",
                "vignette",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn vignette_modifiers_alone_add_no_stage() {
        let edits = Edits {
            vignette: edit_model::Vignette {
                amount: 0.0,
                midpoint: 10.0,
                roundness: -100.0,
                feather: 0.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn sharpening_runs_after_the_vignette_and_before_the_output_transform() {
        // Sharpening a vignetted frame is correct for a ratio-based model: darkened
        // corners sharpen proportionally less in absolute terms.
        let edits = Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "vignette",
                "sharpening",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn sharpening_modifiers_alone_add_no_stage() {
        let edits = Edits {
            sharpening: edit_model::Sharpening {
                amount: 0.0,
                radius: 3.0,
                detail: 0.0,
                masking: 90.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }

    #[test]
    fn local_contrast_runs_after_the_tone_curve_and_before_saturation() {
        // After the tone curve is mandatory: a local contrast computed before it would then
        // be re-mapped by it — the same argument that kept the vignette out of the prefix.
        // Before the colour stages so they see final tonality.
        let edits = Edits {
            exposure: 1.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: 20.0,
            },
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "tone_curve",
                "local_contrast",
                "saturation",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn neutral_local_contrast_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            local_contrast: edit_model::LocalContrast::default(),
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn local_contrast_is_in_the_tail() {
        // It must not join the prefix: it has to be downstream of the tone curve, and
        // putting it upstream would also make every Texture drag invalidate the noise
        // reduction cache.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert!(!prefix.stage_names().contains(&"local_contrast"));
        assert!(tail.stage_names().contains(&"local_contrast"));
    }

    #[test]
    fn tail_kernel_radius_covers_every_spatial_tail_stage() {
        let neutral = Edits::default();
        assert_eq!(tail_kernel_radius(&neutral, RenderScale::FULL), 0);

        // Sharpening alone at its default radius: ceil(3*1.0) + ceil(3*2.0) = 9.
        let sharp = Edits {
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&sharp, RenderScale::FULL), 9);

        // Clarity alone: wide_blur_reach(40.0) = 120, far wider than sharpening ever gets.
        let clarity = Edits {
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 50.0,
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&clarity, RenderScale::FULL), 120);

        // Both: the max, because the two stages are sequential in the pipeline but each
        // reads its own neighbourhood of that stage's input.
        let both = Edits {
            sharpening: sharp.sharpening,
            local_contrast: clarity.local_contrast,
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&both, RenderScale::FULL), 120);

        // The worst case must bound every representable setting.
        let worst = max_tail_kernel_radius(RenderScale::FULL);
        for e in [&neutral, &sharp, &clarity, &both, &all_edits()] {
            assert!(
                tail_kernel_radius(e, RenderScale::FULL) <= worst,
                "an edit set exceeded max_tail_kernel_radius {worst}"
            );
        }
        // And it must be tighter at a smaller render scale, or a preview would over-reserve.
        assert!(max_tail_kernel_radius(RenderScale::new(0.25)) < worst);
    }

    #[test]
    fn geometry_is_prepended_before_other_stages() {
        let edits = Edits {
            exposure: 1.0,
            geometry: edit_model::Geometry {
                angle: 1.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 3);
    }
}
