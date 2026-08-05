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
pub use stages::noise_reduction::NoiseReductionStage;
pub use stages::output_transform::OutputTransformSrgb;
pub use stages::saturation::SaturationStage;
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

    /// Run all stages in order.
    pub fn run(&self, mut image: ImageBuf) -> ImageBuf {
        for stage in &self.stages {
            image = stage.apply(image);
        }
        image
    }
}

/// Build the ordered stage list for a set of edits, rendered at `scale` relative to the
/// full-resolution original. A stage is added only when its params are non-neutral; the
/// output transform is always last.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline {
    let mut stages: Vec<Box<dyn PipelineStage>> = Vec::new();

    // Geometry runs first, so all adjustments operate on the final cropped pixels.
    if edits.geometry != edit_model::Geometry::default() {
        stages.push(Box::new(GeometryStage::new(&edits.geometry)));
    }
    // Noise reduction comes before every adjustment: denoising the source is far easier
    // than denoising after Exposure, Shadows and the tone curve have amplified the grain.
    // `is_active` rather than a `!= default()` check — Detail and Smoothness are
    // modifiers and must not insert a stage on their own.
    if edits.noise_reduction.is_active() {
        stages.push(Box::new(NoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
        )));
    }
    if edits.white_balance != edit_model::WhiteBalance::default() {
        stages.push(Box::new(WhiteBalanceStage::new(&edits.white_balance)));
    }
    if edits.exposure != 0.0 {
        stages.push(Box::new(ExposureStage::new(edits.exposure)));
    }
    if edits.contrast != 0.0 {
        stages.push(Box::new(ContrastStage::new(edits.contrast)));
    }
    if edits.highlights != 0.0 || edits.shadows != 0.0 || edits.whites != 0.0 || edits.blacks != 0.0
    {
        stages.push(Box::new(ToneRegionsStage::new(
            edits.highlights,
            edits.shadows,
            edits.whites,
            edits.blacks,
        )));
    }
    if !edits.tone_curve.points.is_empty() {
        stages.push(Box::new(ToneCurveStage::new(&edits.tone_curve.points)));
    }

    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        stages.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }

    if edits.hsl != edit_model::Hsl::default() {
        stages.push(Box::new(HslStage::new(&edits.hsl)));
    }

    stages.push(Box::new(OutputTransformSrgb));
    Pipeline::new(stages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

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
