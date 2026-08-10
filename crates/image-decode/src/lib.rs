//! Decode non-RAW images (JPEG/PNG/TIFF) into the develop pipeline's working space.
//!
//! An image file is display-referred sRGB; we decode it, normalize to f32 (preserving
//! 8- or 16-bit precision via `to_rgb32f`), and linearize with the sRGB EOTF so the
//! result lives in the same linear-light space as a decoded RAW.

use core_image::{ImageBuf, Thumbnail};
use image::{DynamicImage, ImageDecoder, ImageReader};
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

/// Open `path` with its EXIF orientation already applied.
///
/// ⚠ `image::open` does **not** apply orientation — it hands back the stored pixels, so a
/// phone or camera JPEG shot in portrait comes out on its side. RAW does not have this
/// problem because `raw_decode` applies libraw's `sizes.flip`, which is exactly why portrait
/// RAWs looked right in the grid while portrait JPEGs did not.
///
/// An unreadable or absent orientation tag is treated as "no transform" rather than an
/// error: a missing tag is the overwhelmingly common case, not a failure.
fn open_oriented(path: &Path) -> Result<DynamicImage, ImageDecodeError> {
    let mut decoder = ImageReader::open(path)
        .map_err(image::ImageError::IoError)?
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?
        .into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decode `path` to a linear-light `ImageBuf` (sRGB primaries, sRGB assumed on input).
///
/// Orientation is applied here, not just in [`thumbnail`], so the canvas and the export
/// agree with the grid — the sideways-portrait bug was visible in the browser but was never
/// confined to it.
pub fn decode(path: &Path) -> Result<DecodedImage, ImageDecodeError> {
    let rgb32f = open_oriented(path)?.to_rgb32f(); // samples in [0,1], sRGB-encoded
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
///
/// Orientation is applied *before* the downscale, so the 512 px cap lands on the long edge
/// of the image as it will actually be shown.
pub fn thumbnail(path: &Path) -> Result<Thumbnail, ImageDecodeError> {
    let img = open_oriented(path)?.thumbnail(512, 512).to_rgba8();
    Ok(Thumbnail {
        width: img.width() as usize,
        height: img.height() as usize,
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path no other test will pick.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a monotonic counter — not taken from
    /// the clock. `SystemTime::now()` was not enough: `cargo test` runs these in parallel and
    /// four of them build fixtures through `tagged_jpeg`, one of them eight times in a loop,
    /// so two calls landing in the same clock tick collided and one test deleted the file
    /// another was still reading. That failed roughly one run in five.
    fn temp_path(name: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rustroom_imgdec_{}_{}_{}",
            std::process::id(),
            n,
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

    /// A minimal EXIF APP1 segment carrying just an Orientation tag.
    ///
    /// Hand-built because the `image` crate can read EXIF but not write it, and a fixture
    /// that merely *claims* to be rotated would prove nothing. Layout: little-endian TIFF
    /// header, one IFD entry (tag 0x0112, SHORT, count 1), then a null next-IFD offset.
    fn exif_orientation_app1(orientation: u8) -> Vec<u8> {
        let mut app1 = vec![
            0xFF, 0xE1, 0x00, 0x22, // APP1, length 34 (includes these two length bytes)
            b'E', b'x', b'i', b'f', 0x00, 0x00, // "Exif\0\0"
            b'I', b'I', 0x2A, 0x00, // little-endian, TIFF magic 42
            0x08, 0x00, 0x00, 0x00, // offset of IFD0
            0x01, 0x00, // one entry
            0x12, 0x01, // tag 0x0112 = Orientation
            0x03, 0x00, // type 3 = SHORT
            0x01, 0x00, 0x00, 0x00, // count 1
        ];
        app1.extend_from_slice(&[orientation, 0x00, 0x00, 0x00]); // value, inline
        app1.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // no next IFD
        app1
    }

    /// Write a landscape JPEG — red left half, blue right half — tagged with `orientation`.
    fn tagged_jpeg(name: &str, orientation: u8) -> std::path::PathBuf {
        let (w, h) = (40u32, 20u32);
        let mut img = image::RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let px = if x < w / 2 {
                    image::Rgb([220, 20, 20])
                } else {
                    image::Rgb([20, 20, 220])
                };
                img.put_pixel(x, y, px);
            }
        }
        let plain = temp_path("plain.jpg");
        img.save(&plain).unwrap();
        let bytes = std::fs::read(&plain).unwrap();
        std::fs::remove_file(&plain).ok();

        // Splice the APP1 segment in immediately after SOI (0xFFD8).
        let mut out = Vec::with_capacity(bytes.len() + 36);
        out.extend_from_slice(&bytes[..2]);
        out.extend_from_slice(&exif_orientation_app1(orientation));
        out.extend_from_slice(&bytes[2..]);
        let path = temp_path(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn a_portrait_jpeg_is_rotated_on_decode() {
        // ⚠ The bug this pins: `image::open` returns stored pixels and ignores the
        // orientation tag, so a portrait JPEG stayed on its side — on the canvas and in the
        // export, not only in the grid. RAW never had the problem because `raw_decode`
        // applies libraw's `sizes.flip`.
        let path = tagged_jpeg("rot90.jpg", 6); // EXIF 6 = rotate 90 clockwise
        let d = decode(&path).expect("decode");
        assert_eq!(
            (d.width, d.height),
            (20, 40),
            "a 40x20 source tagged rotate-90 must decode as 20x40"
        );
        // Rotating clockwise puts the source's left (red) half along the top.
        let top = d.image.pixel(10, 5);
        let bottom = d.image.pixel(10, 34);
        assert!(top[0] > top[2], "top should be red, got {top:?}");
        assert!(
            bottom[2] > bottom[0],
            "bottom should be blue, got {bottom:?}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_portrait_jpeg_is_rotated_in_its_thumbnail() {
        // The symptom as reported: portrait JPEGs lay on their side in the grid and strip.
        let path = tagged_jpeg("rot90_thumb.jpg", 6);
        let t = thumbnail(&path).expect("thumb");
        assert!(
            t.height > t.width,
            "thumbnail should be portrait, got {}x{}",
            t.width,
            t.height
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_untagged_jpeg_is_left_alone() {
        // Orientation 1 is "no transform", and a file with no EXIF at all must behave the
        // same — the common case must not be reshaped by the fix.
        let tagged = tagged_jpeg("rot1.jpg", 1);
        let d = decode(&tagged).expect("decode tagged");
        assert_eq!((d.width, d.height), (40, 20));
        std::fs::remove_file(&tagged).ok();

        let bare = temp_path("bare.jpg");
        image::RgbImage::from_pixel(40, 20, image::Rgb([120, 130, 140]))
            .save(&bare)
            .unwrap();
        let d = decode(&bare).expect("decode bare");
        assert_eq!((d.width, d.height), (40, 20));
        std::fs::remove_file(&bare).ok();
    }

    #[test]
    fn every_exif_orientation_is_handled_without_error() {
        // 1..=8 are the whole legal range. A value we mishandled would show up as an error
        // or a nonsensical size rather than being silently ignored.
        for o in 1..=8u8 {
            let path = tagged_jpeg(&format!("o{o}.jpg"), o);
            let d = decode(&path).unwrap_or_else(|e| panic!("orientation {o}: {e}"));
            // 5..=8 involve a 90/270 turn, so they transpose; 1..=4 do not.
            let expect = if (5..=8).contains(&o) {
                (20, 40)
            } else {
                (40, 20)
            };
            assert_eq!((d.width, d.height), expect, "orientation {o}");
            std::fs::remove_file(&path).ok();
        }
    }
}
