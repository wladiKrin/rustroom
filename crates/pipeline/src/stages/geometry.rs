use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::{CropRect, Geometry, Orientation, Quarter};

/// Orient (quarter turns and mirroring), straighten (bilinear rotation about center), then
/// crop to a normalized rectangle.
pub struct GeometryStage {
    orientation: Orientation,
    angle_rad: f32,
    crop: CropRect,
}

impl GeometryStage {
    pub fn new(g: &Geometry) -> Self {
        Self {
            orientation: g.orientation,
            angle_rad: g.angle.to_radians(),
            crop: g.crop,
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
            rotate_bilinear(&image, self.angle_rad)
        } else {
            image
        };
        apply_crop(rotated, &self.crop)
    }
}

/// Bilinearly sample `src` at fractional (sx, sy); out-of-bounds reads as black.
pub(crate) fn sample_bilinear(src: &ImageBuf, sx: f32, sy: f32) -> [f32; 3] {
    let w = src.width() as i32;
    let h = src.height() as i32;
    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let get = |xi: i32, yi: i32| -> [f32; 3] {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            [0.0, 0.0, 0.0]
        } else {
            src.pixel(xi as usize, yi as usize)
        }
    };
    let p00 = get(x0, y0);
    let p10 = get(x0 + 1, y0);
    let p01 = get(x0, y0 + 1);
    let p11 = get(x0 + 1, y0 + 1);
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bot = p01[c] * (1.0 - fx) + p11[c] * fx;
        out[c] = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// Rotate about the image center by `angle_rad`, keeping the canvas size.
fn rotate_bilinear(src: &ImageBuf, angle_rad: f32) -> ImageBuf {
    let w = src.width();
    let h = src.height();
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = angle_rad.sin_cos();
    let mut out = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            // Inverse map: sample the source at -angle.
            let sx = cx + dx * cos + dy * sin;
            let sy = cy - dx * sin + dy * cos;
            out.set_pixel(x, y, sample_bilinear(src, sx, sy));
        }
    }
    out
}

/// Crop to the normalized rectangle; a full-frame rect returns the image unchanged.
fn apply_crop(image: ImageBuf, c: &CropRect) -> ImageBuf {
    let w = image.width();
    let h = image.height();
    let x0 = ((c.left.clamp(0.0, 1.0) * w as f32).round() as usize).min(w);
    let y0 = ((c.top.clamp(0.0, 1.0) * h as f32).round() as usize).min(h);
    let x1 = ((c.right.clamp(0.0, 1.0) * w as f32).round() as usize).clamp(x0 + 1, w);
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h as f32).round() as usize).clamp(y0 + 1, h);
    if x0 == 0 && y0 == 0 && x1 == w && y1 == h {
        return image;
    }
    image.crop(x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        GeometryStage::new(&g).apply(img)
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
        let g = GeometryStage::new(&Geometry::default());
        let mut img = ImageBuf::new(2, 2);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3]);
        let out = g.apply(img.clone());
        assert_eq!(out, img);
    }

    #[test]
    fn crop_changes_dimensions_and_content() {
        let g = GeometryStage::new(&Geometry {
            orientation: Default::default(),
            angle: 0.0,
            crop: CropRect {
                left: 0.5,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
        });
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
        let out = rotate_bilinear(&img, 0.0);
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
        let out = rotate_bilinear(&img, 10f32.to_radians());
        assert_eq!((out.width(), out.height()), (9, 9));
        let [r, _, _] = out.pixel(4, 4);
        assert!((r - 0.4).abs() < 1e-4);
    }
}
