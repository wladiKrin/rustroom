//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

mod stages;
pub use stages::contrast::ContrastStage;
pub use stages::exposure::ExposureStage;
pub use stages::output_transform::OutputTransformSrgb;
pub use stages::tone_curve::ToneCurveStage;
pub use stages::tone_regions::ToneRegionsStage;
pub use stages::white_balance::WhiteBalanceStage;

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

    /// Run all stages in order.
    pub fn run(&self, mut image: ImageBuf) -> ImageBuf {
        for stage in &self.stages {
            image = stage.apply(image);
        }
        image
    }
}

/// Build the ordered stage list for a set of edits. A stage is added only when its
/// params are non-neutral; the output transform is always last.
pub fn build_pipeline(edits: &Edits) -> Pipeline {
    let mut stages: Vec<Box<dyn PipelineStage>> = Vec::new();

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

    stages.push(Box::new(OutputTransformSrgb));
    Pipeline::new(stages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

    #[test]
    fn output_transform_encodes_linear_to_srgb() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.5, 0.5, 0.5]);
        let out = build_pipeline(&Edits::default()).run(img);
        let [r, g, b] = out.pixel(0, 0);
        let expected = color::linear_to_srgb(0.5);
        assert!((r - expected).abs() < 1e-4);
        assert!((g - expected).abs() < 1e-4);
        assert!((b - expected).abs() < 1e-4);
    }

    #[test]
    fn default_pipeline_has_output_transform_last() {
        let p = build_pipeline(&Edits::default());
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn nonneutral_edits_add_stages_before_output_transform() {
        let edits = Edits {
            exposure: 1.0,
            contrast: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits).len(), 3);
    }
}
