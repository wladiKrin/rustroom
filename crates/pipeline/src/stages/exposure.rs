use crate::PipelineStage;
use core_image::ImageBuf;

/// Multiply linear light by `2^ev` (EV stops).
pub struct ExposureStage {
    factor: f32,
}

impl ExposureStage {
    pub fn new(ev: f32) -> Self {
        Self {
            factor: 2.0f32.powf(ev),
        }
    }
}

impl PipelineStage for ExposureStage {
    fn name(&self) -> &'static str {
        "exposure"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_samples(|s| s * self.factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_one_ev_doubles() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        let out = ExposureStage::new(1.0).apply(img);
        let [r, g, b] = out.pixel(0, 0);
        assert!((r - 0.2).abs() < 1e-6);
        assert!((g - 0.4).abs() < 1e-6);
        assert!((b - 0.6).abs() < 1e-6);
    }

    #[test]
    fn zero_ev_is_identity() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.1, 0.5, 0.9]);
        let out = ExposureStage::new(0.0).apply(img);
        assert_eq!(out.pixel(0, 0), [0.1, 0.5, 0.9]);
    }
}
