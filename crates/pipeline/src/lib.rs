//! The develop pipeline: an ordered list of pure `ImageBuf -> ImageBuf` stages,
//! built from an `Edits` op-list. The final stage is always the output transform.

use core_image::ImageBuf;
use edit_model::Edits;

mod stages;
pub use stages::output_transform::OutputTransformSrgb;

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

/// Build the ordered stage list for a set of edits.
/// M1.1: only the output transform. M1.2+ prepends adjustment stages (they run in
/// linear light, before the output transform).
pub fn build_pipeline(_edits: &Edits) -> Pipeline {
    Pipeline::new(vec![Box::new(OutputTransformSrgb)])
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
}
