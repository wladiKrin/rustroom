//! Decode non-RAW images (JPEG/PNG/TIFF) into the develop pipeline's working space.
//!
//! An image file is display-referred sRGB; we decode it, normalize to f32 (preserving
//! 8- or 16-bit precision via `to_rgb32f`), and linearize with the sRGB EOTF so the
//! result lives in the same linear-light space as a decoded RAW.

use core_image::{ImageBuf, Thumbnail};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ImageDecodeError {
    #[error("decoding image: {0}")]
    Decode(#[from] image::ImageError),
}

/// A decoded non-RAW image in the linear working space.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub image: ImageBuf,
    pub width: usize,
    pub height: usize,
}

/// Decode `path` to a linear-light `ImageBuf` (sRGB primaries, sRGB assumed on input).
pub fn decode(path: &Path) -> Result<DecodedImage, ImageDecodeError> {
    let rgb32f = image::open(path)?.to_rgb32f(); // samples in [0,1], sRGB-encoded
    let (w, h) = (rgb32f.width() as usize, rgb32f.height() as usize);
    let data: Vec<f32> = rgb32f
        .into_raw()
        .into_iter()
        .map(color::srgb_to_linear)
        .collect();
    Ok(DecodedImage {
        image: ImageBuf::from_vec(w, h, data),
        width: w,
        height: h,
    })
}

/// Make a display thumbnail (decode + downscale to <=512 px). The file is its own preview.
pub fn thumbnail(path: &Path) -> Result<Thumbnail, ImageDecodeError> {
    let img = image::open(path)?.thumbnail(512, 512).to_rgba8();
    Ok(Thumbnail {
        width: img.width() as usize,
        height: img.height() as usize,
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rustroom_imgdec_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            name
        ))
    }

    #[test]
    fn decode_png_linearizes_srgb() {
        // Solid mid-gray sRGB 128 -> linear ~0.216. PNG is lossless so the value is exact.
        let path = temp_path("gray.png");
        let img = image::RgbImage::from_pixel(4, 3, image::Rgb([128, 128, 128]));
        img.save(&path).unwrap();

        let d = decode(&path).expect("decode png");
        assert_eq!((d.width, d.height), (4, 3));
        assert_eq!(d.image.data().len(), 4 * 3 * 3);
        let v = d.image.pixel(0, 0)[0];
        assert!((v - 0.216).abs() < 0.01, "expected ~0.216 linear, got {v}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn thumbnail_is_downscaled() {
        let path = temp_path("big.png");
        image::RgbImage::from_pixel(1200, 800, image::Rgb([200, 100, 50]))
            .save(&path)
            .unwrap();
        let t = thumbnail(&path).expect("thumb");
        assert!(t.width <= 512 && t.height <= 512 && t.width > 0);
        assert_eq!(t.rgba.len(), t.width * t.height * 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_errors() {
        assert!(decode(Path::new("/no/such/rustroom.png")).is_err());
    }
}
