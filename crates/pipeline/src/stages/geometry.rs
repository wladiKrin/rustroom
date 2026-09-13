use crate::resample::{self, Resampler, Weights};
use crate::{OutRect, PipelineStage};
use core_image::ImageBuf;
use edit_model::{CropRect, Geometry, Orientation, Quarter};
use rayon::prelude::*;

/// Orient (quarter turns and mirroring), straighten (a resampled rotation about the centre),
/// then crop to a normalized rectangle.
pub struct GeometryStage {
    orientation: Orientation,
    angle_rad: f32,
    crop: CropRect,
    /// ⚠ Built once here rather than per pixel: the phase table costs 6144 kernel evaluations.
    weights: Weights,
}

impl GeometryStage {
    pub fn new(g: &Geometry, resampler: Resampler) -> Self {
        Self {
            orientation: g.orientation,
            angle_rad: g.angle.to_radians(),
            crop: g.crop,
            weights: Weights::new(resampler),
        }
    }
}

/// Apply a quarter turn and optional mirror. An index permutation — no resampling, so this is
/// lossless and exact.
///
/// Follows [`Orientation`]'s convention: mirror about the vertical axis first, then turn.
fn orient(src: &ImageBuf, o: Orientation) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let (ow, oh) = if o.rotate.swaps_axes() {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = ImageBuf::new(ow, oh);
    for oy in 0..oh {
        for ox in 0..ow {
            // Inverse map: where in the source does this output pixel come from?
            let (mut sx, sy) = match o.rotate {
                Quarter::Zero => (ox, oy),
                Quarter::Cw90 => (oy, h - 1 - ox),
                Quarter::Cw180 => (w - 1 - ox, h - 1 - oy),
                Quarter::Cw270 => (w - 1 - oy, ox),
            };
            // Un-mirror in *source* space, after un-rotating — the inverse of "mirror first,
            // then turn", so `w` is the source width whatever the turn.
            if o.mirror {
                sx = w - 1 - sx;
            }
            out.set_pixel(ox, oy, src.pixel(sx, sy));
        }
    }
    out
}

impl PipelineStage for GeometryStage {
    fn name(&self) -> &'static str {
        "geometry"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // Orientation first: a lossless whole-image reframe, so the straighten angle and the
        // crop are both expressed in the oriented frame.
        let image = if self.orientation.is_identity() {
            image
        } else {
            orient(&image, self.orientation)
        };
        let rotated = if self.angle_rad != 0.0 {
            rotate(&image, self.angle_rad, &self.weights)
        } else {
            image
        };
        apply_crop(rotated, &self.crop)
    }

    /// ⚠ Orientation can swap the axes and the crop reduces; the rotation keeps the canvas size,
    /// so those are the only two terms. Read through [`crop_bounds`], never re-derived.
    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        let (w, h) = if self.orientation.rotate.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        };
        let (x0, y0, x1, y1) = crop_bounds(w, h, &self.crop);
        if x0 == 0 && y0 == 0 && x1 == w && y1 == h {
            (w, h)
        } else {
            (x1 - x0, y1 - y0)
        }
    }

    fn apply_region(&self, src: &ImageBuf, rect: OutRect) -> Option<ImageBuf> {
        // ⚠ Declines on a non-identity orientation: `orient` is a whole-buffer index permutation
        // feeding the rotation's gather, so region-scoping it needs the permutation folded into
        // the sample coordinates. Uncommon — a camera's portrait orientation is applied at decode
        // through `sizes.flip`, not through `Edits::orientation`.
        if !self.orientation.is_identity() {
            return None;
        }
        let (w, h) = (src.width(), src.height());
        let (x0, y0, x1, y1) = crop_bounds(w, h, &self.crop);
        // ⚠ Refuse a rect this stage could not produce, rather than clamping it: a caller that
        // asked for the wrong thing must fall back to the whole-buffer path, not silently get a
        // different rectangle back without being told.
        if rect.w == 0 || rect.h == 0 || rect.x0 + rect.w > x1 - x0 || rect.y0 + rect.h > y1 - y0 {
            return None;
        }
        let out = OutRect {
            x0: x0 + rect.x0,
            y0: y0 + rect.y0,
            w: rect.w,
            h: rect.h,
        };
        if self.angle_rad == 0.0 {
            // No resampling at all, so a sub-rect of the crop is a sub-rect of the source — and
            // `apply`'s own guard already takes that branch for the whole image.
            return Some(src.crop(out.x0, out.y0, out.w, out.h));
        }
        Some(rotate_region(src, self.angle_rad, &self.weights, out))
    }
}

/// Rotate about the image center by `angle_rad`, keeping the canvas size.
///
/// ⚠⚠ **Parallel over OUTPUT rows, and that is bit-identical rather than merely close.** This is a
/// **gather**: an output pixel reads the whole source and writes only itself, so no two threads
/// write the same byte and none reads another's output. The float operations are therefore the
/// same operations in the same order as the sequential form — which is what lets the 63 goldens
/// stand as the check on it, exactly as they do for `LensCorrectionStage`'s own rayon port.
///
/// ⚠ Measured at a native rung (3123x2085, Catmull-Rom, 8 threads): **160.5 ms sequential against
/// 33.1 ms here, 4.85x**. The sequential form was a deliberate choice that predated anyone
/// measuring it — `perspective.rs` recorded "if this ever needs to be faster, both should move
/// together", and they moved together.
fn rotate(src: &ImageBuf, angle_rad: f32, weights: &Weights) -> ImageBuf {
    rotate_region(
        src,
        angle_rad,
        weights,
        OutRect {
            x0: 0,
            y0: 0,
            w: src.width(),
            h: src.height(),
        },
    )
}

/// Rotate about the **full** image's centre, producing only `rect` of the result.
///
/// ⚠⚠ **`cx` and `cy` are the FULL output's centre, never the sub-rect's.** Using the sub-rect's
/// centre shifts the picture, and the result is a plausible photograph of the wrong part of the
/// frame — the one way to get this wrong.
///
/// ⚠⚠ **[`rotate`] is this function at full extent, which is what makes the whole-buffer and
/// region paths bit-identical BY CONSTRUCTION rather than by comparison.** Two implementations that
/// must agree is how they come to disagree; there is only one here.
fn rotate_region(src: &ImageBuf, angle_rad: f32, weights: &Weights, rect: OutRect) -> ImageBuf {
    let w = src.width();
    let h = src.height();
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = angle_rad.sin_cos();
    const CH: usize = ImageBuf::CHANNELS;
    let stride = rect.w * CH;
    let mut data = vec![0.0f32; rect.w * rect.h * CH];
    // ⚠ `par_chunks_exact_mut(0)` PANICS, and a zero-width image reaches here. The same guard
    // `map_pixels_xy` and `LensCorrectionStage` already carry.
    if stride == 0 {
        return ImageBuf::from_vec(rect.w, rect.h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(ry, orow)| {
            let y = rect.y0 + ry;
            for (rx, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let x = rect.x0 + rx;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                // Inverse map: sample the source at -angle.
                let sx = cx + dx * cos + dy * sin;
                let sy = cy - dx * sin + dy * cos;
                *out = resample::sample(src, sx, sy, weights);
            }
        });
    ImageBuf::from_vec(rect.w, rect.h, data)
}

/// The crop's pixel bounds `(x0, y0, x1, y1)` in an image of `w` x `h`.
///
/// ⚠⚠ **The one rule, read by both [`apply_crop`] and `GeometryStage::output_dims`.** Two
/// derivations of where the crop starts is how a predicted output size and the real one come to
/// disagree — and a region-scoped render would then rasterise every brush mask at the wrong size,
/// which is a wrong picture rather than an error.
///
/// ⚠ Found in passing and deliberately **not** fixed here: a `CropRect` with `left == 1.0` gives
/// `x0 == w`, and `clamp(x0 + 1, w)` then panics because its minimum exceeds its maximum. That is
/// pre-existing and unreachable from the crop tool, which keeps a minimum size; fixing it inside a
/// refactor whose acceptance is "no behaviour moved" would hide the change.
fn crop_bounds(w: usize, h: usize, c: &CropRect) -> (usize, usize, usize, usize) {
    let x0 = ((c.left.clamp(0.0, 1.0) * w as f32).round() as usize).min(w);
    let y0 = ((c.top.clamp(0.0, 1.0) * h as f32).round() as usize).min(h);
    let x1 = ((c.right.clamp(0.0, 1.0) * w as f32).round() as usize).clamp(x0 + 1, w);
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h as f32).round() as usize).clamp(y0 + 1, h);
    (x0, y0, x1, y1)
}

/// Crop to the normalized rectangle; a full-frame rect returns the image unchanged.
fn apply_crop(image: ImageBuf, c: &CropRect) -> ImageBuf {
    let w = image.width();
    let h = image.height();
    let (x0, y0, x1, y1) = crop_bounds(w, h, c);
    if x0 == 0 && y0 == 0 && x1 == w && y1 == h {
        return image;
    }
    image.crop(x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edged_ramp(w: usize, h: usize) -> ImageBuf {
        let mut b = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // ⚠ A hard edge as well as a ramp: every kernel reproduces a linear ramp almost
                // exactly, so a ramp alone cannot tell a shifted sample from a correct one.
                let e = if x * 3 < w { 0.0 } else { 1.0 };
                b.set_pixel(x, y, [x as f32 / w as f32, y as f32 / h as f32, e]);
            }
        }
        b
    }

    /// ⚠ Reports *where* the two differ rather than dumping both buffers — the rule
    /// `gpu-pipeline`'s `assert_same` already follows, after a bare `assert_eq!` on two large
    /// slices once printed 254 MB of numbers instead of a diagnosis.
    fn assert_same(a: &ImageBuf, b: &ImageBuf, what: &str) {
        assert_eq!(
            (a.width(), a.height()),
            (b.width(), b.height()),
            "{what}: sizes differ"
        );
        for (i, (x, y)) in a.data().iter().zip(b.data().iter()).enumerate() {
            assert_eq!(
                x,
                y,
                "{what}: sample {i} of {} differs ({x} vs {y})",
                a.data().len()
            );
        }
    }

    /// ⚠⚠ The acceptance for the whole milestone: a region-scoped geometry must equal the
    /// whole-buffer one cropped, **byte for byte**. Both are the same gather in the same order, so
    /// a tolerance would hide exactly the failure this exists to catch — a rotation centred on the
    /// sub-rect instead of the frame, which produces a plausible photograph of the wrong place.
    #[test]
    fn a_region_of_the_geometry_matches_the_whole_geometry_cropped() {
        let src = edged_ramp(61, 43);
        for g in [
            Geometry {
                angle: 7.0,
                ..Default::default()
            },
            Geometry {
                angle: 0.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.2,
                    right: 0.8,
                    bottom: 0.9,
                },
                ..Default::default()
            },
            Geometry {
                angle: -4.5,
                crop: CropRect {
                    left: 0.15,
                    top: 0.05,
                    right: 0.75,
                    bottom: 0.85,
                },
                ..Default::default()
            },
        ] {
            let stage = GeometryStage::new(&g, Resampler::default());
            let whole = stage.apply(src.clone());
            let (ow, oh) = stage.output_dims(src.width(), src.height());
            assert_eq!(
                (ow, oh),
                (whole.width(), whole.height()),
                "fixture check: output_dims must describe this stage"
            );
            let rect = OutRect {
                x0: 3,
                y0: 5,
                w: ow - 9,
                h: oh - 11,
            };
            let scoped = stage
                .apply_region(&src, rect)
                .expect("fixture check: this geometry must accept a region");
            let want = whole.crop(rect.x0, rect.y0, rect.w, rect.h);
            assert_same(&scoped, &want, &format!("angle {}", g.angle));
        }
    }

    /// ⚠ An orientation is a whole-buffer permutation feeding the gather, so the stage must
    /// decline rather than produce a plausible-looking wrong rectangle.
    #[test]
    fn a_turned_or_mirrored_geometry_declines_a_region() {
        let src = edged_ramp(40, 30);
        for o in [
            Orientation {
                rotate: Quarter::Cw90,
                mirror: false,
            },
            Orientation {
                rotate: Quarter::Zero,
                mirror: true,
            },
        ] {
            let stage = GeometryStage::new(
                &Geometry {
                    angle: 3.0,
                    orientation: o,
                    ..Default::default()
                },
                Resampler::default(),
            );
            assert!(
                stage
                    .apply_region(
                        &src,
                        OutRect {
                            x0: 0,
                            y0: 0,
                            w: 4,
                            h: 4
                        }
                    )
                    .is_none(),
                "{o:?} must decline"
            );
        }
    }

    /// ⚠ A rect this stage could not produce is refused, never clamped — a caller that asked for
    /// the wrong thing must fall back to the whole-buffer path rather than get a different
    /// rectangle back without being told.
    #[test]
    fn a_region_outside_the_output_is_refused() {
        let src = edged_ramp(40, 30);
        let stage = GeometryStage::new(
            &Geometry {
                angle: 2.0,
                ..Default::default()
            },
            Resampler::default(),
        );
        let (ow, oh) = stage.output_dims(40, 30);
        for bad in [
            OutRect {
                x0: 0,
                y0: 0,
                w: ow + 1,
                h: oh,
            },
            OutRect {
                x0: ow - 1,
                y0: 0,
                w: 2,
                h: oh,
            },
            OutRect {
                x0: 0,
                y0: 0,
                w: 0,
                h: oh,
            },
        ] {
            assert!(stage.apply_region(&src, bad).is_none(), "{bad:?}");
        }
    }
    use edit_model::{Orientation, Quarter};

    /// A 3x2 image whose every pixel is distinguishable, so a permutation is checked exactly
    /// rather than by a summary statistic.
    fn ramp() -> ImageBuf {
        let mut img = ImageBuf::new(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                img.set_pixel(x, y, [x as f32, y as f32, 0.0]);
            }
        }
        img
    }

    fn oriented(img: ImageBuf, rotate: Quarter, mirror: bool) -> ImageBuf {
        let g = Geometry {
            orientation: Orientation { rotate, mirror },
            ..Default::default()
        };
        GeometryStage::new(&g, Resampler::default()).apply(img)
    }

    #[test]
    fn orienting_is_an_index_permutation() {
        // ⚠ Named pixels, not a summary: a transpose and a rotation agree on dimensions and on
        // every statistic, and differ only in where individual pixels land.
        let out = oriented(ramp(), Quarter::Cw90, false);
        assert_eq!(
            (out.width(), out.height()),
            (2, 3),
            "a quarter turn swaps axes"
        );
        // Clockwise: the source's bottom-left becomes the output's top-left.
        assert_eq!(out.pixel(0, 0), [0.0, 1.0, 0.0]);
        assert_eq!(out.pixel(1, 0), [0.0, 0.0, 0.0]);
        assert_eq!(out.pixel(1, 2), [2.0, 0.0, 0.0]);

        let out = oriented(ramp(), Quarter::Zero, true);
        assert_eq!((out.width(), out.height()), (3, 2), "a mirror does not");
        assert_eq!(out.pixel(0, 0), [2.0, 0.0, 0.0], "mirrored about x");
        assert_eq!(out.pixel(2, 0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn orienting_twice_by_180_is_the_identity() {
        let src = ramp();
        let once = oriented(src.clone(), Quarter::Cw180, false);
        let twice = oriented(once, Quarter::Cw180, false);
        assert_eq!(twice.data(), src.data(), "bit-identical, no resampling");
    }

    #[test]
    fn the_identity_orientation_changes_nothing() {
        // What keeps all 29 existing goldens byte-identical.
        let src = ramp();
        let out = oriented(src.clone(), Quarter::Zero, false);
        assert_eq!(out.data(), src.data());
    }

    #[test]
    fn zero_angle_full_crop_is_identity() {
        let g = GeometryStage::new(&Geometry::default(), Resampler::default());
        let mut img = ImageBuf::new(2, 2);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        let out = g.apply(img.clone());
        assert_eq!(out, img);
    }

    #[test]
    fn crop_changes_dimensions_and_content() {
        let g = GeometryStage::new(
            &Geometry {
                orientation: Default::default(),
                angle: 0.0,
                crop: CropRect {
                    left: 0.5,
                    top: 0.0,
                    right: 1.0,
                    bottom: 1.0,
                },
            },
            Resampler::default(),
        );
        let mut img = ImageBuf::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                img.set_pixel(x, y, [x as f32, 0.0, 0.0]);
            }
        }
        let out = g.apply(img);
        assert_eq!((out.width(), out.height()), (2, 2));
        assert_eq!(out.pixel(0, 0)[0], 2.0);
    }

    #[test]
    fn rotate_zero_is_identity() {
        let mut img = ImageBuf::new(3, 3);
        img.set_pixel(1, 1, [0.5, 0.5, 0.5]);
        let out = rotate(&img, 0.0, &Weights::new(Resampler::default()));
        assert_eq!(out.pixel(1, 1), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn rotate_preserves_dims_and_uniform_center() {
        let mut img = ImageBuf::new(9, 9);
        for y in 0..9 {
            for x in 0..9 {
                img.set_pixel(x, y, [0.4, 0.4, 0.4]);
            }
        }
        let out = rotate(
            &img,
            10f32.to_radians(),
            &Weights::new(Resampler::default()),
        );
        assert_eq!((out.width(), out.height()), (9, 9));
        let [r, _, _] = out.pixel(4, 4);
        assert!((r - 0.4).abs() < 1e-4);
    }

    /// ⚠ That the choice actually reaches the straighten. The obvious mistake — building the
    /// stage's `Weights` from `Resampler::default()` and ignoring the argument — leaves every
    /// other test in this file green, because they all pass the default.
    #[test]
    fn the_straighten_honours_the_chosen_resampler() {
        // ⚠ A hard edge, not a smooth ramp: every kernel reproduces a linear ramp almost
        // exactly, so a ramp cannot tell two kernels apart.
        let mut img = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = if x < 16 { 0.0 } else { 1.0 };
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let straighten = |k| {
            GeometryStage::new(
                &Geometry {
                    angle: 7.0,
                    ..Default::default()
                },
                k,
            )
            .apply(img.clone())
        };
        let bilinear = straighten(Resampler::Bilinear);
        for k in Resampler::ALL {
            if k == Resampler::Bilinear {
                continue;
            }
            assert_ne!(
                straighten(k).data(),
                bilinear.data(),
                "{k:?} rendered identically to bilinear"
            );
        }
    }
}
