use crate::PipelineStage;
use core_image::ImageBuf;

/// Per-channel gains applied in the working space.
///
/// ⚠⚠ **Takes gains, not a `WhiteBalance`.** Turning Kelvin into gains needs an illuminant
/// *baseline*, and the baseline is a fact about the source — the as-shot temperature for a RAW,
/// D65 for a JPEG — which `pipeline` is source-agnostic about by rule. The conversion therefore
/// happens in `color::wb::adaptation_gains`, called by whoever builds the pipeline and handed the
/// baseline through `RenderSpec`.
///
/// ⚠ The gains are already luminance-normalised by the time they arrive, so a white-balance change
/// moves colour and not brightness. Do not normalise again here.
pub struct WhiteBalanceStage {
    gains: [f32; 3],
}

impl WhiteBalanceStage {
    pub fn new(gains: [f32; 3]) -> Self {
        Self { gains }
    }
}

impl crate::per_pixel::PerPixel for WhiteBalanceStage {
    fn px(&self, _x: usize, _y: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
        let [gr, gg, gb] = self.gains;
        [r * gr, g * gg, b * gb]
    }
}

impl PipelineStage for WhiteBalanceStage {
    fn name(&self) -> &'static str {
        "white_balance"
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

    /// ⚠ Unit gains must be **bit-exactly** the identity, or a photo whose white balance is at its
    /// baseline is not neutral and every golden moves.
    #[test]
    fn unit_gains_are_exactly_the_identity() {
        let s = WhiteBalanceStage::new([1.0, 1.0, 1.0]);
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.2, 0.5, 0.8]);
        img.set_pixel(1, 0, [1.7, 0.0, 0.33]);
        let out = s.apply(img.clone());
        assert_eq!(out.data(), img.data());
    }

    #[test]
    fn the_gains_reach_every_channel() {
        let s = WhiteBalanceStage::new([2.0, 0.5, 4.0]);
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.1, 0.2, 0.05]);
        let [r, g, b] = s.apply(img).pixel(0, 0);
        assert!((r - 0.2).abs() < 1e-6 && (g - 0.1).abs() < 1e-6 && (b - 0.2).abs() < 1e-6);
    }
}
