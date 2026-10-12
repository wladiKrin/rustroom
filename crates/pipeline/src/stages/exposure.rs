use crate::PipelineStage;
use core_image::ImageBuf;

/// Multiply linear light by `2^ev` (EV stops).
pub struct ExposureStage {
    factor: f32,
}

/// The gain an exposure of `ev` stops applies — shared with `SceneOffsetsStage` (M10.12b), so a
/// mask's offset and the global slider are one formula.
pub fn factor(ev: f32) -> f32 {
    2.0f32.powf(ev)
}

impl ExposureStage {
    pub fn new(ev: f32) -> Self {
        Self { factor: factor(ev) }
    }
}

impl crate::per_pixel::PerPixel for ExposureStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        // ⚠ The same per-sample expression `map_samples` applied, to the three channels in order.
        [p[0] * self.factor, p[1] * self.factor, p[2] * self.factor]
    }
}

impl PipelineStage for ExposureStage {
    fn name(&self) -> &'static str {
        "exposure"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::per_pixel::apply_one(self, image)
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<Box<dyn crate::per_pixel::PerPixel + '_>> {
        Some(Box::new(self))
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
