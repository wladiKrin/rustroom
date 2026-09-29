//! Decode non-RAW images (JPEG/PNG/TIFF) into the develop pipeline's working space.
//!
//! An image file is display-referred sRGB; we decode it, normalize to f32 (preserving
//! 8- or 16-bit precision via `to_rgb32f`), linearize with the sRGB EOTF, and convert into
//! the pipeline's linear ProPhoto working space so the
//! result lives in the same linear-light space as a decoded RAW.

mod quick_look;
pub use quick_look::{quick_look, QuickLook};

use core_image::Capture;
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
    // ⚠⚠ Convert into the pipeline's **ProPhoto** working space. Non-RAW input is sRGB by
    // assumption (ICC handling is deferred), and without this the pipeline would read those
    // linear-sRGB values as ProPhoto — every JPEG rendering heavily oversaturated. It is the
    // largest bug this crate can produce and the least visible from inside it, because nothing
    // here looks at colour.
    let image = ImageBuf::from_vec(w, h, data).map_pixels(color::srgb_to_prophoto);
    Ok(DecodedImage {
        image,
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

    #[test]
    fn a_preview_is_the_whole_picture() {
        let dir = std::env::temp_dir().join(format!("rustroom_preview_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        let img = image::RgbaImage::from_fn(30, 20, |x, _| {
            if x < 15 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        img.save(&path).unwrap();
        let p = preview(&path).expect("preview");
        assert_eq!((p.width, p.height), (30, 20));
        assert_eq!(&p.rgba[0..4], &[255, 0, 0, 255], "the first pixel is red");
        assert_eq!(
            &p.rgba[(20 * 4)..(20 * 4 + 4)],
            &[0, 0, 255, 255],
            "pixel 20 is blue"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// ⚠⚠ **THE REAL BYTES FROM A REAL SCANNER, captured 2026-09-24**, not invented: a Fuji
    /// SP-3000 writes `DateTimeOriginal` as this exact placeholder while putting the true date in
    /// `DateTimeDigitized`. Reading only the first tag gave every film scan **no capture date**,
    /// which cost the Library's date grouping, date sorting, the `{date}` token, the exported XMP
    /// and an Immich upload's `fileCreatedAt`.
    #[test]
    fn a_blank_date_placeholder_falls_through_to_the_next_tag() {
        let placeholder = b"    :  :     :  :  ".to_vec();
        let real = b"2025:11:19 13:32:24".to_vec();

        // On its own the placeholder yields nothing — the precondition for the bug.
        assert_eq!(first_parsable_date(&[Some(placeholder.clone())]), None);

        // With a usable tag behind it, the real date is found.
        let got = first_parsable_date(&[Some(placeholder.clone()), Some(real.clone())])
            .expect("the second tag should be used");
        let want = naive_seconds(exif::DateTime::from_ascii(&real).unwrap()).unwrap();
        assert_eq!(got, want);

        // ⚠ Nothing usable anywhere is still `None`, rather than a zero that would file the
        // photograph under 1970.
        assert_eq!(first_parsable_date(&[Some(placeholder), None]), None);
    }

    /// ⚠ The order matters: a real `DateTimeOriginal` must win over the later tags, or a re-saved
    /// file's modification stamp would override its actual capture time.
    #[test]
    fn the_first_usable_date_wins() {
        let first = b"2020:01:02 03:04:05".to_vec();
        let second = b"2025:11:19 13:32:24".to_vec();
        let got = first_parsable_date(&[Some(first.clone()), Some(second)]).unwrap();
        let want = naive_seconds(exif::DateTime::from_ascii(&first).unwrap()).unwrap();
        assert_eq!(got, want, "an earlier candidate must not be skipped");
    }

    #[test]
    fn no_date_candidates_at_all_is_none() {
        assert_eq!(first_parsable_date(&[]), None);
        assert_eq!(first_parsable_date(&[None, None]), None);
    }
}

/// The whole picture at full size, oriented — a JPEG/PNG/TIFF is its own preview (M9.14).
pub fn preview(path: &Path) -> Result<Thumbnail, ImageDecodeError> {
    let img = open_oriented(path)?.to_rgba8();
    Ok(Thumbnail {
        width: img.width() as usize,
        height: img.height() as usize,
        rgba: img.into_raw(),
    })
}

/// Read an image file's EXIF, if it has any.
///
/// ⚠ PNG and TIFF usually carry none. That is not an error: an empty [`Capture`] is a valid
/// answer, and it must still be *recorded*, or the background pass retries the file forever.
pub fn read_capture(path: &Path) -> Result<Capture, ImageDecodeError> {
    // ⚠ `ImageDecodeError` wraps `image::ImageError`, not `io::Error`, so an unopenable file goes
    // through the same "no EXIF is normal" path rather than needing a new error variant.
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(Capture::default());
    };
    let mut r = std::io::BufReader::new(&file);
    let Ok(ex) = exif::Reader::new().read_from_container(&mut r) else {
        // ⚠ No EXIF is normal, not a failure. An error here would make the scanner retry this file
        // on every folder open forever.
        return Ok(Capture::default());
    };
    Ok(capture_of(&ex))
}

/// A parsed EXIF block as a [`Capture`] — shared by [`read_capture`] and [`quick_look`], so the
/// import's fast path and the catalog's scan cannot disagree about a date.
pub(crate) fn capture_of(ex: &exif::Exif) -> Capture {
    let make = string_of(ex, exif::Tag::Make).unwrap_or_default();
    let model = string_of(ex, exif::Tag::Model).unwrap_or_default();
    Capture {
        taken: capture_seconds(ex),
        camera: Capture::camera_name(&make, &model),
        lens: string_of(ex, exif::Tag::LensModel),
        iso: ex
            .get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0)),
        shutter: rational_of(ex, exif::Tag::ExposureTime),
        aperture: rational_of(ex, exif::Tag::FNumber),
        focal_len: rational_of(ex, exif::Tag::FocalLength),
    }
}

/// ⚠⚠ Reads `Field::value`, **never `display_value()`**. EXIF string fields are arrays, and a real
/// X-T4 JPEG returns the lens name followed by **51 empty strings** — `display_value()` renders
/// every one of them, so a naïve implementation stores `"XF23mmF2 R WR", "", "", …`.
fn string_of(ex: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let f = ex.get_field(tag, exif::In::PRIMARY)?;
    let exif::Value::Ascii(parts) = &f.value else {
        return None;
    };
    first_non_empty(parts)
}

/// The first non-empty entry of an EXIF string array.
///
/// ⚠ Split out from `string_of` **so it is testable without building an EXIF container**. The
/// padding is the whole finding, and a test that cannot reach it is not a test of it.
fn first_non_empty(parts: &[Vec<u8>]) -> Option<String> {
    parts
        .iter()
        .map(|b| String::from_utf8_lossy(b).trim().to_string())
        .find(|s| !s.is_empty())
}

fn rational_of(ex: &exif::Exif, tag: exif::Tag) -> Option<f32> {
    let f = ex.get_field(tag, exif::In::PRIMARY)?;
    let v = match &f.value {
        exif::Value::Rational(r) => r.first()?.to_f64(),
        exif::Value::SRational(r) => r.first()?.to_f64(),
        _ => return None,
    };
    (v.is_finite() && v > 0.0).then_some(v as f32)
}

/// The raw ASCII value of `tag`, if the file carries one.
fn ascii_of(ex: &exif::Exif, tag: exif::Tag) -> Option<Vec<u8>> {
    ex.get_field(tag, exif::In::PRIMARY)
        .and_then(|f| match &f.value {
            exif::Value::Ascii(v) => v.first().cloned(),
            _ => None,
        })
}

/// When the shutter fired, from the first date tag that actually parses.
///
/// ⚠⚠ **THREE TAGS IN ORDER, AND THE FALLBACK IS NOT BELT-AND-BRACES — it is the fix for a real
/// bug found by use on 2026-09-24.** A Fuji **SP-3000** film scanner writes
/// `DateTimeOriginal` as the placeholder `"    :  :     :  :  "` — present, blank, unparsable —
/// while putting the real date in **both** `DateTimeDigitized` and `DateTime`. Reading only
/// `DateTimeOriginal` therefore yielded **no capture date at all** for every scan, which silently
/// cost the Library's date grouping, date sorting, the `{date}` filename token, the exported XMP
/// packet and an Immich upload's `fileCreatedAt`. Immich reads the same files correctly, which is
/// how the gap became visible.
///
/// ⚠ The order is deliberate: `DateTimeOriginal` is *the* capture time when it is real, so it wins;
/// `DateTimeDigitized` is when the image was made digital, which for a scan is the closest thing
/// there is; `DateTime` is last because it is the file's own modification stamp and a re-save can
/// move it.
///
/// ⚠ A tag that fails to parse is **skipped rather than fatal**, which is what makes the
/// placeholder fall through instead of poisoning the answer.
fn capture_seconds(ex: &exif::Exif) -> Option<i64> {
    first_parsable_date(&[
        ascii_of(ex, exif::Tag::DateTimeOriginal),
        ascii_of(ex, exif::Tag::DateTimeDigitized),
        ascii_of(ex, exif::Tag::DateTime),
    ])
}

/// The first candidate that parses as a date, as naive wall-clock seconds.
///
/// ⚠ Separated from [`capture_seconds`] so the **ordering** is testable without building an
/// `exif::Exif`, which this crate's tests cannot do.
fn first_parsable_date(candidates: &[Option<Vec<u8>>]) -> Option<i64> {
    candidates
        .iter()
        .flatten()
        .filter_map(|b| exif::DateTime::from_ascii(b).ok())
        .find_map(naive_seconds)
}

/// ⚠ Interpreted as **wall-clock**, matching the RAW reader — EXIF's `DateTimeOriginal` carries no
/// timezone, and a photographer's "the shots from the 17th" means the 17th where they stood.
fn naive_seconds(dt: exif::DateTime) -> Option<i64> {
    chrono::NaiveDate::from_ymd_opt(dt.year as i32, dt.month as u32, dt.day as u32)?
        .and_hms_opt(dt.hour as u32, dt.minute as u32, dt.second as u32)
        .map(|d| d.and_utc().timestamp())
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn a_padded_string_field_is_read_as_one_value() {
        // ⚠⚠ The finding, and it is not hypothetical: for a real X-T4 JPEG, `LensModel` comes back
        // as the lens name followed by **51 empty strings**, and `display_value()` renders every
        // one of them. Reproduced at the level the padding actually lives.
        let mut padded: Vec<Vec<u8>> = vec![b"XF23mmF2 R WR".to_vec()];
        padded.extend(std::iter::repeat_n(Vec::new(), 51));
        assert_eq!(first_non_empty(&padded).as_deref(), Some("XF23mmF2 R WR"));
    }

    #[test]
    fn a_field_of_nothing_but_padding_is_none() {
        let empty: Vec<Vec<u8>> = std::iter::repeat_n(Vec::new(), 8).collect();
        assert_eq!(first_non_empty(&empty), None);
        assert_eq!(first_non_empty(&[]), None);
    }

    #[test]
    fn leading_padding_is_skipped_rather_than_taken() {
        // ⚠ "First non-empty", not "first" — a field whose first slot is blank must not read as
        // absent.
        let parts = vec![Vec::new(), b"  ".to_vec(), b"X-T4".to_vec()];
        assert_eq!(first_non_empty(&parts).as_deref(), Some("X-T4"));
    }

    #[test]
    fn a_wall_clock_conversion_ignores_any_offset() {
        // ⚠ The RAW reader takes `.naive_local()` and this takes the fields as written, so a
        // RAW+JPEG pair of one shot lands on the same second.
        let dt = exif::DateTime {
            year: 2026,
            month: 5,
            day: 17,
            hour: 19,
            minute: 26,
            second: 41,
            nanosecond: None,
            offset: Some(120),
        };
        let with_offset = naive_seconds(dt);
        let dt2 = exif::DateTime {
            offset: None,
            ..exif::DateTime {
                year: 2026,
                month: 5,
                day: 17,
                hour: 19,
                minute: 26,
                second: 41,
                nanosecond: None,
                offset: None,
            }
        };
        assert_eq!(with_offset, naive_seconds(dt2));
        assert!(with_offset.is_some());
    }
}

/// The image's displayed pixel dimensions, from the header alone.
///
/// ⚠ Cheap — `image_dimensions` reads only the header, unlike the RAW side, which must read the
/// whole file.
///
/// ⚠⚠ **EXIF orientation is applied**, so a portrait JPEG reports portrait. `image_dimensions`
/// does not do that on its own, and without it a RAW and its JPEG sibling would disagree about
/// which way up the photograph is.
///
/// ⚠ They still differ slightly in *size*, and that is correct: measured on an X-T4 pair, the RAW
/// is 6246×4170 and the camera JPEG 6240×4160, because libraw's active area includes border
/// pixels the camera crops. Unlike camera and date — facts about the *shot*, normalized to agree
/// exactly — dimensions are a property of the *file*.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    use image::metadata::Orientation;
    let (w, h) = image::image_dimensions(path).ok()?;
    // ⚠ The four transposing orientations, named rather than a numeric range: the enum is what
    // `open_oriented` already uses, and a `5..=8` range would silently mean something else if the
    // crate ever reordered it.
    let transposed = ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.into_decoder().ok())
        .and_then(|mut d| d.orientation().ok())
        .is_some_and(|o| {
            matches!(
                o,
                Orientation::Rotate90
                    | Orientation::Rotate270
                    | Orientation::Rotate90FlipH
                    | Orientation::Rotate270FlipH
            )
        });
    Some(if transposed { (h, w) } else { (w, h) })
}
