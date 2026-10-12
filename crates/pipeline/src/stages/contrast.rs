use crate::PipelineStage;
use core_image::ImageBuf;

/// Linear middle grey — the value contrast pivots around.
///
/// ⚠ `pub` so `gpu-pipeline`'s shader can **substitute** it rather than transcribe it. A
/// transcribed constant is checked for agreeing; a substituted one **cannot disagree**, which is
/// the distinction `luma_wgsl`'s doc records as making it stronger than `oklab_wgsl`. `color`'s
/// own doc records six copies of a Rec.709 triple that all went wrong on one day.
pub const PIVOT: f32 = 0.18;

/// Contrast around a mid-gray pivot: `out = pivot + (in - pivot) * slope`.
pub struct ContrastStage {
    amount: f32,
}

impl ContrastStage {
    /// `amount` ~[-100, 100]; 0 = identity, +100 = slope 2, -100 = slope 0 (flat).
    pub fn new(amount: f32) -> Self {
        Self { amount }
    }
}

/// Contrast `amount` on one pixel — shared with `SceneOffsetsStage` (M10.12b), so a mask's offset
/// and the global slider are one formula.
pub fn apply(p: [f32; 3], amount: f32) -> [f32; 3] {
    let slope = 1.0 + amount / 100.0;
    let f = |s: f32| (PIVOT + (s - PIVOT) * slope).max(0.0);
    [f(p[0]), f(p[1]), f(p[2])]
}

impl crate::per_pixel::PerPixel for ContrastStage {
    fn px(&self, _x: usize, _y: usize, p: [f32; 3]) -> [f32; 3] {
        apply(p, self.amount)
    }
}

impl PipelineStage for ContrastStage {
    fn name(&self) -> &'static str {
        "contrast"
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
