//! Encode a display-referred (sRGB, 0..=1) `ImageBuf` to an image file.

use core_image::ImageBuf;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("image encoding failed: {0}")]
    Encode(#[from] image::ImageError),
}

/// Quantize a display-encoded image to interleaved 8-bit RGB, clamping to `[0, 1]`.
pub fn to_rgb8(image: &ImageBuf) -> Vec<u8> {
    image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// Save as JPEG at the given quality (1..=100). Image must already be display-encoded.
pub fn save_jpeg(image: &ImageBuf, path: &Path, quality: u8) -> Result<(), ExportError> {
    let buf = to_rgb8(image);
    let rgb = image::RgbImage::from_raw(image.width() as u32, image.height() as u32, buf)
        .expect("buffer size matches dimensions");
    let file =
        std::io::BufWriter::new(std::fs::File::create(path).map_err(image::ImageError::IoError)?);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
    enc.encode_image(&rgb)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    #[test]
    fn to_rgb8_quantizes_and_clamps() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        let bytes = to_rgb8(&img);
        assert_eq!(bytes, vec![0, 128, 255, 0, 255, 64]);
    }

    #[test]
    fn save_jpeg_writes_a_file() {
        let mut img = ImageBuf::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        let path = std::env::temp_dir().join("oslr_export_test.jpg");
        save_jpeg(&img, &path, 90).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        let _ = std::fs::remove_file(&path);
    }
}
