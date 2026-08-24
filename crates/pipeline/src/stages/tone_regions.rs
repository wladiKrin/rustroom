use crate::PipelineStage;
use core_image::ImageBuf;

const STRENGTH: f32 = 0.5; // scales all four amounts

/// Highlights/shadows/whites/blacks as a single luminance-region-weighted gain.
/// All amounts ~[-100, 100]; positive brightens that region. Hue-preserving.
pub struct ToneRegionsStage {
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
}

impl ToneRegionsStage {
    pub fn new(highlights: f32, shadows: f32, whites: f32, blacks: f32) -> Self {
        Self {
            highlights: highlights / 100.0,
            shadows: shadows / 100.0,
            whites: whites / 100.0,
            blacks: blacks / 100.0,
        }
    }

    fn gain_for_luma(&self, l: f32) -> f32 {
        let l = l.clamp(0.0, 1.0);
        let shadow_w = (1.0 - l) * (1.0 - l);
        let highlight_w = l * l;
        let black_w = (1.0 - 3.0 * l).max(0.0); // only l < 1/3
        let white_w = (3.0 * l - 2.0).max(0.0); // only l > 2/3
        1.0 + STRENGTH
            * (self.shadows * shadow_w
                + self.highlights * highlight_w
                + self.blacks * black_w
                + self.whites * white_w)
    }
}

impl PipelineStage for ToneRegionsStage {
    fn name(&self) -> &'static str {
        "tone_regions"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_pixels(|[r, g, b]| {
            let l = color::luminance([r, g, b]);
            let gain = self.gain_for_luma(l).max(0.0);
            [r * gain, g * gain, b * gain]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_is_identity() {
        let s = ToneRegionsStage::new(0.0, 0.0, 0.0, 0.0);
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [0.2, 0.5, 0.8]);
        let out = s.apply(img);
        let [r, g, b] = out.pixel(0, 0);
        assert!((r - 0.2).abs() < 1e-6 && (g - 0.5).abs() < 1e-6 && (b - 0.8).abs() < 1e-6);
    }

    #[test]
    fn positive_shadows_brighten_darks_more_than_lights() {
        let s = ToneRegionsStage::new(0.0, 100.0, 0.0, 0.0);
        assert!(s.gain_for_luma(0.05) > s.gain_for_luma(0.9));
        assert!(s.gain_for_luma(0.9) >= 1.0);
    }

    #[test]
    fn positive_highlights_brighten_lights_more_than_darks() {
        let s = ToneRegionsStage::new(100.0, 0.0, 0.0, 0.0);
        assert!(s.gain_for_luma(0.95) > s.gain_for_luma(0.1));
    }
}
