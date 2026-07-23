use crate::PipelineStage;
use core_image::ImageBuf;

/// Final stage: encode linear-light samples to sRGB for display/export.
pub struct OutputTransformSrgb;

impl PipelineStage for OutputTransformSrgb {
    fn name(&self) -> &'static str {
        "output_transform_srgb"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_samples(color::linear_to_srgb)
    }
}
