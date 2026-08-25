//! Optical corrections from a lens profile: distortion, TCA and vignetting.
//!
//! ⚠ Runs **first in the prefix, before `GeometryStage`**, because the calibration is defined in
//! the uncropped, unrotated sensor frame. Being in the prefix also means it always runs
//! whole-buffer, so it needs no `Region` and no kernel margin despite being a spatial resample.

use crate::PipelineStage;
use core_image::ImageBuf;
use lens_profile::Corrector;

/// Bilinearly sample `src` at fractional `(sx, sy)`, one channel; outside the frame reads black.
///
/// ⚠ Per channel rather than per pixel, because TCA gives each channel its own source point.
fn sample_channel(src: &ImageBuf, sx: f32, sy: f32, ch: usize) -> f32 {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
    let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
    let at = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            src.pixel(x as usize, y as usize)[ch]
        }
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
    let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bot * fy
}

pub struct LensCorrectionStage {
    corrector: Corrector,
}

impl LensCorrectionStage {
    pub fn new(corrector: Corrector) -> Self {
        Self { corrector }
    }
}

impl PipelineStage for LensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let src = self.corrector.source(fx, fy);
                let gain = self.corrector.gain(fx, fy);
                let mut px = [0.0f32; 3];
                for (ch, p) in px.iter_mut().enumerate() {
                    *p = sample_channel(&image, src[ch][0], src[ch][1], ch) * gain;
                }
                out.set_pixel(x, y, px);
            }
        }
        out
    }
}
