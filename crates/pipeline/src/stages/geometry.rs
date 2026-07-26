use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::{CropRect, Geometry};

/// Straighten (bilinear rotation about center) then crop to a normalized rectangle.
pub struct GeometryStage {
    angle_rad: f32,
    crop: CropRect,
}

impl GeometryStage {
    pub fn new(g: &Geometry) -> Self {
        Self {
            angle_rad: g.angle.to_radians(),
            crop: g.crop,
        }
    }
}

impl PipelineStage for GeometryStage {
    fn name(&self) -> &'static str {
        "geometry"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let rotated = if self.angle_rad != 0.0 {
            rotate_bilinear(&image, self.angle_rad)
        } else {
            image
        };
        apply_crop(rotated, &self.crop)
    }
}

/// Bilinearly sample `src` at fractional (sx, sy); out-of-bounds reads as black.
fn sample_bilinear(src: &ImageBuf, sx: f32, sy: f32) -> [f32; 3] {
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
