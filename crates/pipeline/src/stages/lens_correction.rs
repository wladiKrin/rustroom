//! Optical corrections from a lens profile: distortion, TCA and vignetting.
//!
//! ⚠ Runs **first in the prefix, before `GeometryStage`**, because the calibration is defined in
//! the uncropped, unrotated sensor frame. Being in the prefix also means it always runs
//! whole-buffer, so it needs no `Region` and no kernel margin despite being a spatial resample.

use crate::PipelineStage;
use core_image::ImageBuf;
use lens_profile::{Corrections, Corrector, LensShot};

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
    lens: LensShot,
    want: Corrections,
}

impl LensCorrectionStage {
    pub fn new(lens: LensShot, want: Corrections) -> Self {
        Self { lens, want }
    }

    /// The correction for an image of these dimensions.
    ///
    /// ⚠ Built here rather than handed in, because a [`Corrector`] is tied to a specific image
    /// size and this is the only place that knows it — the preview and the 1:1 view render at
    /// different sizes through the same `Edits`. Handing one in would make a size mismatch
    /// *representable*, and it would be silently wrong rather than an error. Building it is
    /// cheap: the database is already parsed, so this is a `Modifier::new` and three `enable_*`.
    fn corrector(&self, w: usize, h: usize) -> Option<Corrector> {
        Corrector::new(&self.lens.profile, &self.lens.shot, w, h, self.want)
    }
}

impl PipelineStage for LensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        // Nothing enabled, or nothing this profile can supply: pass the image through untouched.
        let Some(corrector) = self.corrector(w, h) else {
            return image;
        };
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let src = corrector.source(fx, fy);
                let gain = corrector.gain(fx, fy);
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
