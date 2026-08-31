//! Manual perspective correction — the keystone transform.
//!
//! ⚠ Runs in the **prefix**, between `LensCorrectionStage` and `GeometryStage`. After lens
//! correction because a calibration is defined in the uncropped sensor frame and describes the
//! optics rather than the pose; before geometry because you correct the keystone, level what is
//! left, then crop away the corners the correction emptied. Running it after the crop would move
//! content inside a rectangle already chosen, leaving gaps *within* the crop.
//!
//! ⚠ Being in the prefix it always runs whole-buffer — so **no `Region` and no kernel margin**,
//! despite being a spatial resample. And being normalised it takes **no `RenderScale`**: the
//! preview and the export agree exactly, the creative vignette's property.
//!
//! ⚠⚠ It sits **upstream of the brush's anchor space**, which is `GeometryStage`'s input. That is
//! what means `FrameMap` needs no change at all. The cost is the caveat lens correction already
//! documents: toggling perspective after painting shifts strokes.

use crate::perspective::Homography;
use crate::stages::geometry::sample_bilinear;
use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::Perspective;

pub struct PerspectiveStage {
    params: Perspective,
}

impl PerspectiveStage {
    pub fn new(params: Perspective) -> Self {
        Self { params }
    }
}

impl PipelineStage for PerspectiveStage {
    fn name(&self) -> &'static str {
        "perspective"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        // ⚠⚠ The neutral case returns the image **untouched**, and that is not merely an
        // optimisation. The homography is the identity to within f32 rounding, not exactly — so
        // resampling through it shifts samples by ~1e-7 and bilinear interpolation then blends
        // neighbours, softening the whole frame for nothing. `GeometryStage` guards its rotation
        // and its crop the same way and for the same reason.
        //
        // ⚠ `build_prefix_steps` already skips an inactive stage, so this is unreachable in
        // production today. It is here so that constructing the stage unconditionally stays safe.
        if !self.params.is_active() || w < 2 || h < 2 {
            return image;
        }
        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        // Half-height is the unit, so `x` runs ±aspect and `y` runs ±1.
        let aspect = cx / cy;
        let hom = Homography::from_edits(&self.params, aspect);

        // ⚠ Sequential, mirroring `rotate_bilinear` in the sibling stage rather than reaching for
        // rayon. Geometry's whole-buffer rotation measures 203 ms at full resolution and nobody
        // has complained; matching it keeps the two readable side by side. If this ever needs to
        // be faster, both should move together.
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                let s = hom.source_of(n);
                out.set_pixel(x, y, sample_bilinear(&image, cx + s.0 * cy, cy + s.1 * cy));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineStage;

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    /// ⚠ Bit-identical, not merely close. A neutral stage that *nearly* reproduced its input
    /// would move all 47 goldens, and the failure would read as a maths bug somewhere else.
    #[test]
    fn a_neutral_perspective_reproduces_the_image_exactly() {
        let src = ramp(37, 23);
        let out = PerspectiveStage::new(Perspective::default()).apply(src.clone());
        assert_eq!(out, src);
    }

    /// ⚠ That the stage actually reaches the pixels. The obvious sabotage — returning the input
    /// untouched — leaves everything else in this file green.
    #[test]
    fn a_keystone_really_moves_the_pixels() {
        let src = ramp(64, 48);
        let out = PerspectiveStage::new(Perspective {
            vertical: 50.0,
            ..Default::default()
        })
        .apply(src.clone());
        assert_ne!(out, src);
        assert_eq!((out.width(), out.height()), (src.width(), src.height()));
    }

    /// The canvas size is preserved, which is what lets `FrameMap` stay untouched:
    /// `GeometryStage` receives exactly the dimensions it would have without this stage.
    #[test]
    fn the_canvas_size_is_unchanged_by_a_strong_correction() {
        for (w, h) in [(64, 48), (48, 64), (2, 2)] {
            let out = PerspectiveStage::new(Perspective {
                vertical: 100.0,
                horizontal: -100.0,
                aspect: 80.0,
                scale: 0.0,
            })
            .apply(ramp(w, h));
            assert_eq!((out.width(), out.height()), (w, h));
        }
    }

    /// A degenerate image must pass through rather than divide by zero.
    #[test]
    fn a_one_pixel_image_passes_through() {
        let src = ImageBuf::new(1, 1);
        let out = PerspectiveStage::new(Perspective {
            vertical: 50.0,
            ..Default::default()
        })
        .apply(src.clone());
        assert_eq!(out, src);
    }
}
