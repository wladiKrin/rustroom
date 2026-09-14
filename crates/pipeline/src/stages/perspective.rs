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
use crate::resample::{self, Resampler, Weights};
use crate::{PipelineStage, Rect};
use core_image::ImageBuf;
use edit_model::Perspective;
use rayon::prelude::*;

pub struct PerspectiveStage {
    params: Perspective,
    /// ⚠ Built once here rather than per pixel, like `GeometryStage`'s.
    weights: Weights,
}

impl PerspectiveStage {
    pub fn new(params: Perspective, resampler: Resampler) -> Self {
        Self {
            params,
            weights: Weights::new(resampler),
        }
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
        self.render(&image, Rect { x: 0, y: 0, w, h })
    }

    fn apply_region(&self, src: &ImageBuf, rect: Rect) -> Option<ImageBuf> {
        let (w, h) = (src.width(), src.height());
        // ⚠ Perspective preserves dimensions, so the output rect is bounded by the input's.
        if rect.w == 0 || rect.h == 0 || rect.x + rect.w > w || rect.y + rect.h > h {
            return None;
        }
        if !self.params.is_active() || w < 2 || h < 2 {
            // Mirrors `apply`'s neutral guard: the stage is the identity, so the region is a plain
            // sub-rect and resampling through it would soften the frame for nothing.
            return Some(src.crop(rect.x, rect.y, rect.w, rect.h));
        }
        Some(self.render(src, rect))
    }
}

impl PerspectiveStage {
    /// The gather, producing only `rect` of the output.
    ///
    /// ⚠⚠ **`cx`, `cy` and `aspect` are the FULL output's, never the sub-rect's** — the homography
    /// is defined about the frame's centre in units of its half-height, so a sub-rect's own centre
    /// would describe a different map and render a plausible photograph of the wrong place.
    ///
    /// ⚠⚠ **`apply` is this at full extent**, so the whole-buffer and region paths are
    /// bit-identical by construction rather than by comparison.
    ///
    /// ⚠⚠ **Parallel over OUTPUT rows, and bit-identical rather than merely close.** This is a
    /// **gather**: an output pixel reads the whole source and writes only itself, so no two
    /// threads write the same byte and none reads another's output — the float operations are
    /// the same operations in the same order as the sequential form, which is what lets the 63
    /// goldens stand as the check on it.
    ///
    /// ⚠ It moved together with `geometry::rotate`, which is what the note that used to sit
    /// here asked for: *"if this ever needs to be faster, both should move together."*
    /// Measured at a native rung (3123x2085, Catmull-Rom, 8 threads): **158.8 ms sequential
    /// against 30.2 ms here, 5.26x**, beside geometry's 4.85x.
    fn render(&self, image: &ImageBuf, rect: Rect) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        // Half-height is the unit, so `x` runs ±aspect and `y` runs ±1.
        let aspect = cx / cy;
        let hom = Homography::from_edits(&self.params, aspect);

        const CH: usize = ImageBuf::CHANNELS;
        let stride = rect.w * CH;
        let mut data = vec![0.0f32; rect.w * rect.h * CH];
        // ⚠ `par_chunks_exact_mut(0)` PANICS. Unreachable behind `apply`'s `w < 2` guard and
        // `apply_region`'s zero check, and kept because those are about other cases.
        if stride == 0 {
            return ImageBuf::from_vec(rect.w, rect.h, data);
        }
        data.par_chunks_exact_mut(stride)
            .enumerate()
            .for_each(|(ry, orow)| {
                let y = rect.y + ry;
                for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                    let x = rect.x + rx;
                    let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                    let s = hom.source_of(n);
                    *out = resample::sample(image, cx + s.0 * cy, cy + s.1 * cy, &self.weights);
                }
            });
        ImageBuf::from_vec(rect.w, rect.h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineStage;

    /// ⚠ The same acceptance geometry's carries, for the same reason: both are gathers, so
    /// equality is byte-for-byte and a tolerance would hide a map centred on the wrong point.
    #[test]
    fn a_region_of_the_perspective_matches_the_whole_perspective_cropped() {
        let mut src = ImageBuf::new(61, 43);
        for y in 0..43 {
            for x in 0..61 {
                // ⚠ A hard edge as well as a ramp: every kernel reproduces a linear ramp almost
                // exactly, so a ramp alone cannot tell a shifted sample from a correct one.
                let e = if x * 3 < 61 { 0.0 } else { 1.0 };
                src.set_pixel(x, y, [x as f32 / 61.0, y as f32 / 43.0, e]);
            }
        }
        let p = Perspective {
            vertical: 30.0,
            ..Default::default()
        };
        let stage = PerspectiveStage::new(p, Resampler::default());
        let whole = stage.apply(src.clone());
        let rect = Rect {
            x: 7,
            y: 4,
            w: 21,
            h: 17,
        };
        let scoped = stage
            .apply_region(&src, rect)
            .expect("fixture check: an active perspective must accept a region");
        let want = whole.crop(rect.x, rect.y, rect.w, rect.h);
        assert_eq!(
            (scoped.width(), scoped.height()),
            (want.width(), want.height())
        );
        for (i, (a, b)) in scoped.data().iter().zip(want.data().iter()).enumerate() {
            assert_eq!(
                a,
                b,
                "sample {i} of {} differs ({a} vs {b})",
                scoped.data().len()
            );
        }
    }

    /// ⚠ A rect the stage could not produce is refused, never clamped — the caller must fall back
    /// to the whole-buffer path rather than get a different rectangle back without being told.
    #[test]
    fn a_perspective_region_outside_the_output_is_refused() {
        let src = ramp(40, 30);
        let stage = PerspectiveStage::new(
            Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            Resampler::default(),
        );
        for bad in [
            Rect {
                x: 0,
                y: 0,
                w: 41,
                h: 30,
            },
            Rect {
                x: 39,
                y: 0,
                w: 2,
                h: 30,
            },
            Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 30,
            },
        ] {
            assert!(stage.apply_region(&src, bad).is_none(), "{bad:?}");
        }
    }

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
        let out =
            PerspectiveStage::new(Perspective::default(), Resampler::default()).apply(src.clone());
        assert_eq!(out, src);
    }

    /// ⚠ That the stage actually reaches the pixels. The obvious sabotage — returning the input
    /// untouched — leaves everything else in this file green.
    #[test]
    fn a_keystone_really_moves_the_pixels() {
        let src = ramp(64, 48);
        let out = PerspectiveStage::new(
            Perspective {
                vertical: 50.0,
                ..Default::default()
            },
            Resampler::default(),
        )
        .apply(src.clone());
        assert_ne!(out, src);
        assert_eq!((out.width(), out.height()), (src.width(), src.height()));
    }

    /// The canvas size is preserved, which is what lets `FrameMap` stay untouched:
    /// `GeometryStage` receives exactly the dimensions it would have without this stage.
    #[test]
    fn the_canvas_size_is_unchanged_by_a_strong_correction() {
        for (w, h) in [(64, 48), (48, 64), (2, 2)] {
            let out = PerspectiveStage::new(
                Perspective {
                    vertical: 100.0,
                    horizontal: -100.0,
                    aspect: 80.0,
                    scale: 0.0,
                },
                Resampler::default(),
            )
            .apply(ramp(w, h));
            assert_eq!((out.width(), out.height()), (w, h));
        }
    }

    /// A degenerate image must pass through rather than divide by zero.
    #[test]
    fn a_one_pixel_image_passes_through() {
        let src = ImageBuf::new(1, 1);
        let out = PerspectiveStage::new(
            Perspective {
                vertical: 50.0,
                ..Default::default()
            },
            Resampler::default(),
        )
        .apply(src.clone());
        assert_eq!(out, src);
    }

    /// ⚠ That the choice reaches this stage too. Lens correction, geometry and perspective each
    /// build their own `Weights`, so each needs its own guard — one of the three honouring the
    /// setting says nothing about the other two.
    #[test]
    fn the_keystone_honours_the_chosen_resampler() {
        // ⚠ A hard edge, not `ramp`: every kernel reproduces a linear ramp almost exactly.
        let mut src = ImageBuf::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let v = if (x / 4) % 2 == 0 { 0.0 } else { 1.0 };
                src.set_pixel(x, y, [v, v, v]);
            }
        }
        let keystone = |k| {
            PerspectiveStage::new(
                Perspective {
                    vertical: 40.0,
                    ..Default::default()
                },
                k,
            )
            .apply(src.clone())
        };
        let bilinear = keystone(Resampler::Bilinear);
        for k in Resampler::ALL {
            if k == Resampler::Bilinear {
                continue;
            }
            assert_ne!(
                keystone(k).data(),
                bilinear.data(),
                "{k:?} rendered identically to bilinear"
            );
        }
    }
}
