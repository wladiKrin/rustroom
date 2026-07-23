use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::WhiteBalance;

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722]; // Rec.709 luma weights

/// Creative white balance: temp/tint -> luminance-normalized per-channel gains.
pub struct WhiteBalanceStage {
    gains: [f32; 3],
}

impl WhiteBalanceStage {
    pub fn new(wb: &WhiteBalance) -> Self {
        let t = wb.temp / 100.0;
        let g = wb.tint / 100.0;
        // Raw gains: warmer -> more R, less B; magenta(+tint) -> less G.
        let mut gains = [1.0 + 0.4 * t, 1.0 - 0.3 * g, 1.0 - 0.4 * t];
        // Normalize so a neutral gray keeps its luminance (WB shifts color, not brightness).
        let lum = LUMA[0] * gains[0] + LUMA[1] * gains[1] + LUMA[2] * gains[2];
        if lum > 1e-6 {
            for c in &mut gains {
                *c /= lum;
            }
        }
        Self { gains }
    }
}

impl PipelineStage for WhiteBalanceStage {
    fn name(&self) -> &'static str {
        "white_balance"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let [gr, gg, gb] = self.gains;
        image.map_pixels(|[r, g, b]| [r * gr, g * gg, b * gb])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_wb_is_identity() {
        let s = WhiteBalanceStage::new(&WhiteBalance::default());
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.2, 0.5, 0.8]);
        let out = s.apply(img);
        let [r, g, b] = out.pixel(0, 0);
        assert!((r - 0.2).abs() < 1e-5 && (g - 0.5).abs() < 1e-5 && (b - 0.8).abs() < 1e-5);
    }

    #[test]
    fn warming_raises_red_lowers_blue() {
        let s = WhiteBalanceStage::new(&WhiteBalance {
            temp: 50.0,
            tint: 0.0,
        });
        assert!(s.gains[0] > s.gains[2]);
    }

    #[test]
    fn neutral_gray_preserves_luminance() {
        let s = WhiteBalanceStage::new(&WhiteBalance {
            temp: 60.0,
            tint: -20.0,
        });
        let gray = [0.5, 0.5, 0.5];
        let out_lum: f32 = (0..3).map(|i| LUMA[i] * gray[i] * s.gains[i]).sum();
        let in_lum: f32 = (0..3).map(|i| LUMA[i] * gray[i]).sum();
        assert!((out_lum - in_lum).abs() < 1e-4);
    }
}
