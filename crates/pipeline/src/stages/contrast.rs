use crate::PipelineStage;
use core_image::ImageBuf;

const PIVOT: f32 = 0.18; // linear middle gray

/// Contrast around a mid-gray pivot: `out = pivot + (in - pivot) * slope`.
pub struct ContrastStage {
    slope: f32,
}

impl ContrastStage {
    /// `amount` ~[-100, 100]; 0 = identity, +100 = slope 2, -100 = slope 0 (flat).
    pub fn new(amount: f32) -> Self {
        Self {
            slope: 1.0 + amount / 100.0,
        }
    }
}

impl PipelineStage for ContrastStage {
    fn name(&self) -> &'static str {
        "contrast"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let slope = self.slope;
        image.map_samples(|s| (PIVOT + (s - PIVOT) * slope).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.05, 0.18, 0.9]);
        let out = ContrastStage::new(0.0).apply(img);
        let [r, g, b] = out.pixel(0, 0);
        assert!((r - 0.05).abs() < 1e-6 && (g - 0.18).abs() < 1e-6 && (b - 0.9).abs() < 1e-6);
    }

    #[test]
    fn pivot_is_a_fixed_point() {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [PIVOT, PIVOT, PIVOT]);
        let out = ContrastStage::new(80.0).apply(img);
        let [r, _, _] = out.pixel(0, 0);
        assert!((r - PIVOT).abs() < 1e-6);
    }

    #[test]
    fn positive_contrast_pushes_darks_down_lights_up() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.05, 0.05, 0.05]);
        img.set_pixel(1, 0, [0.6, 0.6, 0.6]);
        let out = ContrastStage::new(50.0).apply(img);
        assert!(out.pixel(0, 0)[0] < 0.05);
        assert!(out.pixel(1, 0)[0] > 0.6);
    }
}
