//! Encode a display-referred (sRGB, 0..=1) `ImageBuf` to an image file.

use core_image::ImageBuf;
use image::{ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
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

/// What to encode, and at what depth.
///
/// **Flat rather than `format` + `bit_depth`.** There is no such thing as a 16-bit JPEG, and a
/// separate depth field would need a rule saying so; flattening makes the invalid state
/// unrepresentable — the same reasoning behind `Decision::Cancel` carrying no `apply_to_all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Png8,
    Png16,
    Tiff8,
    Tiff16,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::Jpeg,
        ExportFormat::Png8,
        ExportFormat::Png16,
        ExportFormat::Tiff8,
        ExportFormat::Tiff16,
    ];

    /// ⚠ `tif` not `tiff`, `jpg` not `jpeg` — fixed so existing exports keep their shape.
    /// Both spellings are equally valid; this one is simply chosen.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png8 | ExportFormat::Png16 => "png",
            ExportFormat::Tiff8 | ExportFormat::Tiff16 => "tif",
        }
    }

    /// Whether `quality` means anything. Only JPEG is lossy, and this is what greys the
    /// quality slider out rather than leaving it implying it does something.
    pub fn is_lossy(self) -> bool {
        matches!(self, ExportFormat::Jpeg)
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Png8 => "PNG 8-bit",
            ExportFormat::Png16 => "PNG 16-bit",
            ExportFormat::Tiff8 => "TIFF 8-bit",
            ExportFormat::Tiff16 => "TIFF 16-bit",
        }
    }
}

/// Quantize a display-encoded image to interleaved 16-bit RGB, clamping to `[0, 1]`.
///
/// The extra bits are **real**: the pipeline is `f32` end to end and libraw hands us 16-bit
/// data, so a 16-bit file carries precision an 8-bit one discards. Returns `Vec<u16>` rather
/// than bytes so it stays directly testable; the encoder wants native-endian bytes.
pub fn to_rgb16(image: &ImageBuf) -> Vec<u16> {
    image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect()
}

/// Native-endian bytes of [`to_rgb16`], which is what `image`'s `Rgb16` encoder expects.
fn rgb16_ne_bytes(image: &ImageBuf) -> Vec<u8> {
    to_rgb16(image)
        .into_iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}

/// Encode a display-referred image. `quality` is ignored unless the format is lossy.
///
/// One entry point rather than a function per format, so no caller can branch on format by
/// accident.
pub fn save(
    image: &ImageBuf,
    path: &Path,
    format: ExportFormat,
    quality: u8,
) -> Result<(), ExportError> {
    let (w, h) = (image.width() as u32, image.height() as u32);
    let file =
        std::io::BufWriter::new(std::fs::File::create(path).map_err(image::ImageError::IoError)?);

    match format {
        ExportFormat::Jpeg => {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png8 => {
            let enc = image::codecs::png::PngEncoder::new(file);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png16 => {
            let enc = image::codecs::png::PngEncoder::new(file);
            enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
        }
        ExportFormat::Tiff8 => {
            let enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff16 => {
            let enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
        }
    }
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

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1).max(1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        // Structural uniqueness, not clock-based: `cargo test` runs these in parallel and
        // this codebase has already had a flaky test from two of them sharing a temp path.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("rustroom_export_{}_{i}_{name}", std::process::id()))
    }

    #[test]
    fn to_rgb16_quantizes_and_clamps() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        assert_eq!(to_rgb16(&img), vec![0, 32768, 65535, 0, 65535, 16384]);
    }

    #[test]
    fn extensions_and_lossiness_are_fixed() {
        assert_eq!(ExportFormat::Jpeg.extension(), "jpg");
        assert_eq!(ExportFormat::Png8.extension(), "png");
        assert_eq!(ExportFormat::Png16.extension(), "png");
        assert_eq!(ExportFormat::Tiff8.extension(), "tif");
        assert_eq!(ExportFormat::Tiff16.extension(), "tif");
        for f in ExportFormat::ALL {
            assert_eq!(f.is_lossy(), f == ExportFormat::Jpeg);
        }
        assert_eq!(ExportFormat::default(), ExportFormat::Jpeg);
    }

    #[test]
    fn every_format_writes_a_readable_file() {
        let img = ramp(8, 4);
        for f in ExportFormat::ALL {
            let path = tmp(&format!("all.{}", f.extension()));
            save(&img, &path, f, 90).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let back = image::open(&path).unwrap_or_else(|e| panic!("{f:?} reopen: {e}"));
            assert_eq!((back.width(), back.height()), (8, 4), "{f:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn sixteen_bit_output_really_carries_more_than_eight() {
        // ⚠ The test that matters. A silent 8-bit downgrade passes every other test here —
        // the file still opens, still has the right size, still looks right. This writes a
        // value 8 bits cannot represent and checks it survived the round trip.
        //
        // 1/512 = 0.001953: x65535 = 128, but x255 = 0.498 which rounds to 0.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [1.0 / 512.0, 1.0 / 512.0, 1.0 / 512.0]);

        for f in [ExportFormat::Png16, ExportFormat::Tiff16] {
            let path = tmp(&format!("deep.{}", f.extension()));
            save(&img, &path, f, 90).unwrap();
            let back = image::open(&path).unwrap().to_rgb16();
            assert_eq!(back.get_pixel(0, 0)[0], 128, "{f:?} lost precision");
            let _ = std::fs::remove_file(&path);
        }

        // And the 8-bit formats genuinely cannot: the same value rounds to 0.
        for f in [ExportFormat::Png8, ExportFormat::Tiff8] {
            let path = tmp(&format!("shallow.{}", f.extension()));
            save(&img, &path, f, 90).unwrap();
            let back = image::open(&path).unwrap().to_rgb8();
            assert_eq!(back.get_pixel(0, 0)[0], 0, "{f:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn export_format_round_trips_json() {
        // The name is part of the on-disk format for settings and export presets.
        for f in ExportFormat::ALL {
            let back: ExportFormat =
                serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
            assert_eq!(f, back);
        }
        assert_eq!(
            serde_json::to_string(&ExportFormat::Tiff16).unwrap(),
            "\"Tiff16\""
        );
    }
}
