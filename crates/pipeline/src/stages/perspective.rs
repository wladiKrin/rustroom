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
        self.render(&crate::RegionSrc::whole(&image), Rect { x: 0, y: 0, w, h })
    }

    fn apply_region(&self, src: &crate::RegionSrc<'_>, rect: Rect) -> Option<ImageBuf> {
        // ⚠⚠ The FULL image's dimensions, never the slice in hand.
        let (w, h) = src.full;
        // ⚠ Perspective preserves dimensions, so the output rect is bounded by the input's.
        if rect.w == 0 || rect.h == 0 || rect.x + rect.w > w || rect.y + rect.h > h {
            return None;
        }
        if !self.params.is_active() || w < 2 || h < 2 {
            // Mirrors `apply`'s neutral guard: the stage is the identity, so the region is a plain
            // sub-rect and resampling through it would soften the frame for nothing.
            // ⚠ Local coordinates for the buffer copy: `rect` is in full-image space.
            return Some(src.buf.crop(
                rect.x - src.origin.0,
                rect.y - src.origin.1,
                rect.w,
                rect.h,
            ));
        }
        Some(self.render(src, rect))
    }

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
    /// ⚠ A homography sends lines to lines, so the image of a rectangle is a quadrilateral and
    /// the bounding box of its four mapped corners contains it **exactly** — the same argument as
    /// geometry's rotation, which is affine.
    ///
    /// ⚠⚠ **Except across the horizon**, where the denominator vanishes and the image is
    /// unbounded. `is_finite` on every mapped corner is the guard, and it **declines** — failing
    /// toward the whole-buffer path, where a wrong answer costs milliseconds rather than pixels.
    ///
    /// ⚠ The tap radius comes from `self.weights.kernel()` rather than a stored `Resampler`, so
    /// the weights and the radius cannot disagree. Geometry's rule.
    fn input_rect_for(&self, out: Rect, in_w: usize, in_h: usize) -> Option<Rect> {
        if out.w == 0 || out.h == 0 || in_w < 2 || in_h < 2 {
            return None;
        }
        if !self.params.is_active() {
            // Dimension-preserving and an exact identity, so the rect is its own pre-image.
            return Some(out);
        }
        let cx = (in_w as f32 - 1.0) / 2.0;
        let cy = (in_h as f32 - 1.0) / 2.0;
        let hom = Homography::from_edits(&self.params, cx / cy);
        let (mut lo_x, mut lo_y) = (f32::INFINITY, f32::INFINITY);
        let (mut hi_x, mut hi_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        let xs = [out.x as f32, (out.x + out.w - 1) as f32];
        let ys = [out.y as f32, (out.y + out.h - 1) as f32];
        for &y in &ys {
            for &x in &xs {
                let n = ((x - cx) / cy, (y - cy) / cy);
                let sp = hom.source_of(n);
                let sx = cx + sp.0 * cy;
                let sy = cy + sp.1 * cy;
                // ⚠⚠ **Defence in depth, and the sabotage for it is INERT — recorded rather
                // than covered by an invented test.** A homography's denominator vanishes at the
                // horizon and the image of the rect is then unbounded, so this would be
                // load-bearing for an unbounded transform. It is unreachable for any
                // representable `Perspective`: `MAX_ANGLE_DEG` is 30, so the denominator vanishes
                // at y = -1/tan(30 deg) ~= -1.73 half-heights while the frame spans y in [-1, 1].
                // Disabling this leaves all 473 tests green, which is a fact about the parameter
                // bounds rather than about the guard. It becomes live the moment `MAX_ANGLE_DEG`
                // passes 45.
                if !sx.is_finite() || !sy.is_finite() {
                    return None;
                }
                lo_x = lo_x.min(sx);
                hi_x = hi_x.max(sx);
                lo_y = lo_y.min(sy);
                hi_y = hi_y.max(sy);
            }
        }
        let pad = self.weights.kernel().radius();
        let x = (lo_x.floor().max(0.0) as usize).saturating_sub(pad);
        let y = (lo_y.floor().max(0.0) as usize).saturating_sub(pad);
        let bx = ((hi_x.ceil().max(0.0) as usize) + pad + 1).min(in_w);
        let by = ((hi_y.ceil().max(0.0) as usize) + pad + 1).min(in_h);
        Some(Rect {
            x,
            y,
            w: bx.saturating_sub(x),
            h: by.saturating_sub(y),
        })
    }
}

impl PerspectiveStage {
    fn render(&self, image: &crate::RegionSrc<'_>, rect: Rect) -> ImageBuf {
        // ⚠⚠ The FULL image's dimensions. The homography's normalisation is a property of the
        // frame; taking these from `image.buf` renormalises onto the region and produces a
        // plausible photograph of the wrong part of it.
        let (w, h) = image.full;
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
                    *out =
                        resample::sample_region(image, cx + s.0 * cy, cy + s.1 * cy, &self.weights);
                }
            });
        ImageBuf::from_vec(rect.w, rect.h, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineStage;

    #[test]
    fn perspective_back_projects_every_sample_it_will_read() {
        // The same containment property geometry's carries, against the real inverse map.
        let p = Perspective {
            vertical: 40.0,
            ..Default::default()
        };
        let s = PerspectiveStage::new(p, Resampler::CatmullRom);
        let (w, h) = (400usize, 300usize);
        let out = Rect {
            x: 100,
            y: 80,
            w: 50,
            h: 40,
        };
        let got = s.input_rect_for(out, w, h).unwrap();

        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        let hom = Homography::from_edits(&p, cx / cy);
        let pad = Resampler::CatmullRom.radius() as f32;
        for oy in out.y..out.y + out.h {
            for ox in out.x..out.x + out.w {
                let n = ((ox as f32 - cx) / cy, (oy as f32 - cy) / cy);
                let sp = hom.source_of(n);
                let (sx, sy) = (cx + sp.0 * cy, cy + sp.1 * cy);
                for (tx, ty) in [(sx - pad, sy - pad), (sx + pad, sy + pad)] {
                    let tx = tx.clamp(0.0, w as f32 - 1.0);
                    let ty = ty.clamp(0.0, h as f32 - 1.0);
                    assert!(
                        tx >= got.x as f32 - 1.0
                            && tx <= (got.x + got.w) as f32
                            && ty >= got.y as f32 - 1.0
                            && ty <= (got.y + got.h) as f32,
                        "sample ({tx}, {ty}) for output ({ox}, {oy}) falls outside {got:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_degenerate_frame_declines_rather_than_producing_a_rect() {
        // ⚠⚠ A homography's denominator vanishes at the horizon and the image of the rect is then
        // unbounded; `is_finite` is the guard and it fails toward the whole-buffer path. This
        // fixture reaches the *other* refusal — a frame too small to normalise — and the horizon
        // branch itself is recorded as defence in depth below.
        let s = PerspectiveStage::new(
            Perspective {
                vertical: 100.0,
                ..Default::default()
            },
            Resampler::CatmullRom,
        );
        assert_eq!(
            s.input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1
                },
                1,
                1
            ),
            None
        );
        assert_eq!(
            s.input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0
                },
                400,
                300
            ),
            None
        );
    }

    #[test]
    fn an_inactive_perspective_asks_for_exactly_the_rect_it_was_given() {
        let s = PerspectiveStage::new(Perspective::default(), Resampler::CatmullRom);
        let out = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert_eq!(s.input_rect_for(out, 200, 200), Some(out));
    }

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
            .apply_region(&crate::RegionSrc::whole(&src), rect)
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
            assert!(
                stage
                    .apply_region(&crate::RegionSrc::whole(&src), bad)
                    .is_none(),
                "{bad:?}"
            );
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
