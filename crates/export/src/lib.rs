//! Encode a display-referred (sRGB, 0..=1) `ImageBuf` to an image file.

pub mod density;
pub mod dng;
pub mod dng_hdr;
pub mod lj92;
pub mod metadata;
pub mod output_sharpen;
pub mod size_limit;
pub mod sizing;
pub(crate) mod tiff_ifd;
pub mod watermark;

pub use metadata::{ExportMetadata, FileMetadata, MetadataScope};
pub use output_sharpen::OutputSharpening;
pub use watermark::{Logo, Watermark, WatermarkCorner, WatermarkKind};

use core_image::ImageBuf;
use image::{ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("image encoding failed: {0}")]
    Encode(#[from] image::ImageError),
    /// A container this crate wrote turned out not to have the shape it must have, or the
    /// metadata does not fit in it. ⚠ Its own variant rather than a stringly `Encode`, because
    /// nothing here went through `image` — the file is edited byte by byte.
    #[error("could not write metadata: {0}")]
    Metadata(String),
}

/// Quantize a display-encoded image to interleaved 8-bit RGB, clamping to `[0, 1]`.
///
/// Kept as a free function with this name so no call site changes, but the quantization itself
/// now lives on `ImageBuf` — one implementation for the encoder and for the preview canvas,
/// which therefore cannot drift. `to_rgb8_quantizes_and_clamps` below is the guard that this
/// delegation preserved the behaviour exactly.
pub fn to_rgb8(image: &ImageBuf) -> Vec<u8> {
    image.to_rgb8()
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
    /// The finished picture as a linear DNG (M9.10) — see `dng`.
    Dng,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Jpeg,
        ExportFormat::Png8,
        ExportFormat::Png16,
        ExportFormat::Tiff8,
        ExportFormat::Tiff16,
        ExportFormat::Dng,
    ];

    /// The colour space this format is always rendered in, whatever the export's setting —
    /// `Some` for DNG alone, which stores linear ProPhoto and lets the opening application convert.
    /// ⚠ The export render and the dialog both read this, so the two cannot disagree.
    pub fn fixed_space(self) -> Option<color::OutputSpace> {
        match self {
            ExportFormat::Dng => Some(color::OutputSpace::ProPhoto),
            _ => None,
        }
    }

    /// ⚠ `tif` not `tiff`, `jpg` not `jpeg` — fixed so existing exports keep their shape.
    /// Both spellings are equally valid; this one is simply chosen.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png8 | ExportFormat::Png16 => "png",
            ExportFormat::Tiff8 | ExportFormat::Tiff16 => "tif",
            ExportFormat::Dng => "dng",
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
            ExportFormat::Dng => "DNG (linear, rendered)",
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

/// The ICC profile for an output space, serialized once per space.
///
/// ⚠ sRGB uses lcms2's own built-in profile, which is canonical; the other three are constructed
/// from primaries, a white point and a transfer curve.
///
/// ⚠⚠ **An ICC tone curve runs device→linear**, which is the *inverse* of what
/// `OutputSpace::encode` applies. Getting this backwards produces a self-consistently wrong
/// file: right primaries, opens fine, washed out in a way easily mistaken for a rendering
/// preference. `the_profile_agrees_with_our_own_maths` is the guard, and it is the only test
/// here that can see it.
///
/// Falls back to an empty slice if lcms2 ever fails, because an export that writes an untagged
/// file is far better than one that refuses to write at all.
pub fn icc_for(space: color::OutputSpace) -> &'static [u8] {
    use color::OutputSpace as S;
    static SRGB: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static P3: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static ADOBE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static PROPHOTO: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

    match space {
        S::Srgb => {
            SRGB.get_or_init(|| pin_created(lcms2::Profile::new_srgb().icc().unwrap_or_default()))
        }
        S::DisplayP3 => P3.get_or_init(|| {
            build_icc(
                D65,
                [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
                srgb_curve(),
                "Display P3",
            )
        }),
        S::AdobeRgb => ADOBE.get_or_init(|| {
            build_icc(
                D65,
                [(0.640, 0.330), (0.210, 0.710), (0.150, 0.060)],
                lcms2::ToneCurve::new(563.0 / 256.0),
                "Adobe RGB (1998) compatible",
            )
        }),
        S::ProPhoto => PROPHOTO.get_or_init(|| {
            build_icc(
                // ⚠ ProPhoto is a **D50** space. The other three are D65; this is not an
                // inconsistency to unify.
                (0.3457, 0.3585),
                [
                    (0.734699, 0.265301),
                    (0.159597, 0.840403),
                    (0.036598, 0.000105),
                ],
                prophoto_curve(),
                "ProPhoto RGB compatible",
            )
        }),
    }
    .as_slice()
}

const D65: (f64, f64) = (0.3127, 0.3290);

/// The sRGB transfer curve, in ICC's device→linear direction.
///
/// ICC parametric type 4 is `Y = (aX + b)^g` for `X >= d`, `Y = cX` below.
fn srgb_curve() -> lcms2::ToneCurve {
    lcms2::ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
        .expect("sRGB parametric curve")
}

/// ROMM's transfer curve — gamma 1.8 with a linear toe — in ICC's device→linear direction.
fn prophoto_curve() -> lcms2::ToneCurve {
    // The encoded threshold is 16 x (1/512) = 0.03125, and the toe's device->linear slope is 1/16.
    lcms2::ToneCurve::new_parametric(4, &[1.8, 1.0, 0.0, 1.0 / 16.0, 0.03125])
        .expect("ProPhoto parametric curve")
}

/// ⚠ `name` is not decoration. Without a description tag every constructed profile reports as
/// "RGB built-in" — macOS `sips`, Preview and Photoshop all show that — so the three would be
/// indistinguishable to anyone inspecting an exported file, and unverifiable by hand. Found by
/// checking a real export rather than by a test.
///
/// The names say "compatible" because these are profiles built from the same primaries and
/// curve, not Adobe's or Apple's own redistributed files.
fn build_icc(
    white: (f64, f64),
    primaries: [(f64, f64); 3],
    curve: lcms2::ToneCurve,
    name: &str,
) -> Vec<u8> {
    let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
    let prim = lcms2::CIExyYTRIPLE {
        Red: xy(primaries[0]),
        Green: xy(primaries[1]),
        Blue: xy(primaries[2]),
    };
    let Ok(mut profile) = lcms2::Profile::new_rgb(&xy(white), &prim, &[&curve, &curve, &curve])
    else {
        return Vec::new();
    };
    let mut mlu = lcms2::MLU::new(1);
    mlu.set_text_ascii(name, lcms2::Locale::none());
    profile.write_tag(
        lcms2::TagSignature::ProfileDescriptionTag,
        lcms2::Tag::MLU(&mlu),
    );
    pin_created(profile.icc().unwrap_or_default())
}

/// When every exported profile says it was made: 2026-08-24 00:00:00 — the day these profiles were
/// first written into exports (colour management piece B).
///
/// ⚠⚠ **Pinned, not lcms2's "now" (M10.7a).** A profile built at run time carries the time it was
/// built, so two exports of one photo differed across runs and `cmp` could not say whether a change
/// moved a pixel (found 2026-09-01). A **valid** date rather than zeros: month 0 and day 0 are not a
/// legal ICC `dateTimeNumber`, and validators flag them.
pub const ICC_CREATED: [u16; 6] = [2026, 8, 24, 0, 0, 0];

/// `icc` with its header's creation date — bytes 24..36, six big-endian `u16`s — set to
/// [`ICC_CREATED`]. Anything shorter than an ICC header (128 bytes) is returned untouched; the profile
/// ID at 84..100 is left alone, and lcms2 leaves it zero, so nothing else depends on the date.
fn pin_created(mut icc: Vec<u8>) -> Vec<u8> {
    if icc.len() >= 128 {
        for (i, v) in ICC_CREATED.iter().enumerate() {
            icc[24 + 2 * i..26 + 2 * i].copy_from_slice(&v.to_be_bytes());
        }
    }
    icc
}

/// Encode a display-referred image. `quality` is ignored unless the format is lossy.
///
/// One entry point rather than a function per format, so no caller can branch on format by
/// accident. The raster formats go through [`encode`]; a DNG is written here.
pub fn save(
    image: &ImageBuf,
    path: &Path,
    format: ExportFormat,
    quality: u8,
    space: color::OutputSpace,
) -> Result<(), ExportError> {
    if format == ExportFormat::Dng {
        // ⚠ The render arrives display-ENCODED in `space` (ProPhoto, when it came through the
        // export render — `fixed_space`); a DNG stores LINEAR data, so the encoding is undone
        // and, for any other space, its primaries converted back to the working space.
        return dng::save(&dng::linear_working(image, space), path);
    }
    let bytes = encode(image, format, quality, space, None)?;
    std::fs::write(path, bytes).map_err(image::ImageError::IoError)?;
    Ok(())
}

/// Encode a display-referred image into the bytes of a raster file, carrying `ppi` when given
/// (M10.10b).
///
/// ⚠ In memory rather than to a path, so a file-size limit can measure an encode before anything
/// is written, and the PPI and the XMP can be added without reading the file back. `None` writes
/// exactly what `save` always wrote. ⚠ A DNG is `Err` — it is written by [`save`], and carries no PPI.
pub fn encode(
    image: &ImageBuf,
    format: ExportFormat,
    quality: u8,
    space: color::OutputSpace,
    ppi: Option<f32>,
) -> Result<Vec<u8>, ExportError> {
    let (w, h) = (image.width() as u32, image.height() as u32);
    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        ExportFormat::Jpeg => {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
            enc.set_icc_profile(icc_for(space).to_vec())
                .map_err(image::ImageError::Unsupported)?;
            if let Some(ppi) = ppi {
                enc.set_pixel_density(density::jpeg_density(ppi));
            }
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png8 | ExportFormat::Png16 => {
            let mut enc = image::codecs::png::PngEncoder::new(&mut out);
            enc.set_icc_profile(icc_for(space).to_vec())
                .map_err(image::ImageError::Unsupported)?;
            if format == ExportFormat::Png8 {
                enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
            } else {
                enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
            }
        }
        ExportFormat::Tiff8 | ExportFormat::Tiff16 => {
            let mut enc = image::codecs::tiff::TiffEncoder::new(&mut out);
            enc.set_icc_profile(icc_for(space).to_vec())
                .map_err(image::ImageError::Unsupported)?;
            if format == ExportFormat::Tiff8 {
                enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
            } else {
                enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
            }
        }
        ExportFormat::Dng => {
            return Err(ExportError::Metadata(
                "a DNG is written by `save`, not encoded in memory".into(),
            ))
        }
    }
    let bytes = out.into_inner();
    // ⚠ PNG and TIFF take the PPI after the encode — `image` offers no hook for either; the JPEG
    // took it above, in its JFIF header.
    match (format, ppi) {
        (ExportFormat::Png8 | ExportFormat::Png16, Some(ppi)) => density::png_with_ppi(&bytes, ppi),
        (ExportFormat::Tiff8 | ExportFormat::Tiff16, Some(ppi)) => {
            density::tiff_with_ppi(&bytes, ppi)
        }
        _ => Ok(bytes),
    }
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
        test_dir::path("export").join(name)
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

    /// A written file's size and 8-bit pixels, through the reader that format has — `image`, or
    /// libraw for a DNG, which `image` cannot open (M9.10).
    pub(crate) fn reopen(path: &std::path::Path, f: ExportFormat) -> (u32, u32, Vec<u8>) {
        if f == ExportFormat::Dng {
            let d = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene())
                .unwrap_or_else(|e| panic!("{f:?} reopen: {e}"));
            let px = to_rgb8(&d.image);
            return (d.image.width() as u32, d.image.height() as u32, px);
        }
        let back = image::open(path).unwrap_or_else(|e| panic!("{f:?} reopen: {e}"));
        (back.width(), back.height(), back.to_rgb8().into_raw())
    }

    /// ⚠ 32×24, not smaller: libraw refuses any image under 22 px on a side
    /// (`identify.cpp`: `height < 22 || width < 22`), so a DNG fixture below that is unreadable
    /// for a reason that has nothing to do with the file.
    #[test]
    fn every_format_writes_a_readable_file() {
        let img = ramp(32, 24);
        for f in ExportFormat::ALL {
            let path = tmp(&format!("all.{}", f.extension()));
            save(&img, &path, f, 90, color::OutputSpace::Srgb)
                .unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let (w, h, _) = reopen(&path, f);
            assert_eq!((w, h), (32, 24), "{f:?}");
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
            save(&img, &path, f, 90, color::OutputSpace::Srgb).unwrap();
            let back = image::open(&path).unwrap().to_rgb16();
            assert_eq!(back.get_pixel(0, 0)[0], 128, "{f:?} lost precision");
            let _ = std::fs::remove_file(&path);
        }

        // And the 8-bit formats genuinely cannot: the same value rounds to 0.
        for f in [ExportFormat::Png8, ExportFormat::Tiff8] {
            let path = tmp(&format!("shallow.{}", f.extension()));
            save(&img, &path, f, 90, color::OutputSpace::Srgb).unwrap();
            let back = image::open(&path).unwrap().to_rgb8();
            assert_eq!(back.get_pixel(0, 0)[0], 0, "{f:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Whether `path` carries our sRGB profile.
    ///
    /// ⚠ Two instruments, because neither works for every container — and finding that out
    /// was the interesting part of this feature:
    ///
    /// - **PNG** stores `iCCP` **zlib-compressed**, so the profile bytes are not literally
    ///   present in the file. Its decoder has to be asked.
    /// - **`image`'s TIFF decoder cannot read back the ICC tag it just wrote** — it looks for
    ///   the tag with `get_tag_u8_vec`, which does not match the type `write_tag` emits. That
    ///   is a round-trip gap in the crate, **not** a failure to embed: the bytes are in the
    ///   file, and macOS `sips` reports `profile: sRGB built-in` for every format we write.
    ///
    /// So: decoder for PNG, byte search for JPEG and TIFF. Asking the decoder everywhere
    /// would have reported a bug that does not exist.
    fn carries_profile(path: &std::path::Path, format: ExportFormat) -> bool {
        let bytes = std::fs::read(path).expect("written file");
        match format {
            ExportFormat::Png8 | ExportFormat::Png16 => {
                use image::ImageDecoder;
                let mut dec = image::ImageReader::open(path)
                    .unwrap()
                    .with_guessed_format()
                    .unwrap()
                    .into_decoder()
                    .unwrap();
                dec.icc_profile()
                    .unwrap()
                    .is_some_and(|p| p.as_slice() == icc_for(color::OutputSpace::Srgb))
            }
            // 'acsp' is the signature every ICC profile carries at offset 36.
            _ => bytes
                .windows(icc_for(color::OutputSpace::Srgb).len())
                .any(|w| w == icc_for(color::OutputSpace::Srgb)),
        }
    }

    /// An ICC header's creation date, bytes 24..36: year, month, day, hours, minutes, seconds.
    fn created(icc: &[u8]) -> [u16; 6] {
        std::array::from_fn(|i| u16::from_be_bytes([icc[24 + 2 * i], icc[25 + 2 * i]]))
    }

    /// ⚠⚠ M10.7a: every exported profile says it was made on `ICC_CREATED`, whatever the clock
    /// says, so two exports of one photo are byte-identical across runs. ⚠ Proven different first
    /// (standing lesson (c)): lcms2's own profile carries the time it was built, and a pin that did
    /// nothing would pass the loop below on the one day it happened to match.
    #[test]
    fn every_exported_profile_carries_the_pinned_creation_date() {
        let fresh = lcms2::Profile::new_srgb().icc().expect("lcms2 builds sRGB");
        assert_ne!(
            created(&fresh),
            ICC_CREATED,
            "fixture: lcms2 stamps the build time, not the pinned date"
        );
        for s in [
            color::OutputSpace::Srgb,
            color::OutputSpace::DisplayP3,
            color::OutputSpace::AdobeRgb,
            color::OutputSpace::ProPhoto,
        ] {
            assert_eq!(created(icc_for(s)), ICC_CREATED, "{s:?}");
        }
    }

    /// The pin moves the date and nothing else — every changed byte is inside the header's date
    /// field — and leaves something too short to be a profile alone.
    #[test]
    fn pinning_touches_only_the_creation_date() {
        let fresh = lcms2::Profile::new_srgb().icc().expect("lcms2 builds sRGB");
        let pinned = pin_created(fresh.clone());
        assert_eq!(pinned.len(), fresh.len());
        let moved: Vec<usize> = (0..fresh.len())
            .filter(|&i| fresh[i] != pinned[i])
            .collect();
        assert!(
            !moved.is_empty(),
            "fixture: the fresh date was already the pinned one"
        );
        assert!(moved.iter().all(|i| (24..36).contains(i)), "{moved:?}");
        assert_eq!(pin_created(vec![1, 2, 3]), vec![1, 2, 3]);
    }

    #[test]
    fn every_format_embeds_an_srgb_profile() {
        // ⚠ A 16-bit TIFF is an archival format; shipping it untagged means the consumer
        // guesses. JPEG is included because it had been shipping untagged all along, and an
        // inconsistency nobody can explain is worse than either choice.
        assert!(
            !icc_for(color::OutputSpace::Srgb).is_empty(),
            "the profile should not be empty"
        );
        assert_eq!(
            &icc_for(color::OutputSpace::Srgb)[36..40],
            b"acsp",
            "lcms2 did not return a well-formed ICC profile"
        );

        let img = ramp(8, 4);
        for f in ExportFormat::ALL {
            let path = tmp(&format!("icc.{}", f.extension()));
            save(&img, &path, f, 90, color::OutputSpace::Srgb).unwrap();
            if f == ExportFormat::Dng {
                // ⚠ A DNG describes its colour with matrices, not an ICC profile: the check is
                // that `ColorMatrix1` (tag 50721, `0xC621`) is there — its correctness is
                // `dng::tests::libraw_reads_the_dng_back_as_the_picture`.
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    bytes.windows(2).any(|w| w == [0x21, 0xC6]),
                    "DNG has no ColorMatrix1"
                );
            } else {
                assert!(carries_profile(&path, f), "{f:?} carries no ICC profile");
            }
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

    #[test]
    fn every_space_produces_a_distinct_non_empty_profile() {
        // ⚠ `icc_for` falls back to an empty slice if lcms2 fails, so "non-empty" is a real check
        // rather than a formality — a silent failure would ship untagged files that look fine
        // until someone opens them on a wide-gamut display.
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for s in color::OutputSpace::ALL {
            let icc = icc_for(s);
            assert!(!icc.is_empty(), "{} produced no profile", s.label());
            assert!(
                !seen.contains(&icc.to_vec()),
                "{} produced a duplicate profile",
                s.label()
            );
            seen.push(icc.to_vec());
        }
    }

    #[test]
    fn the_profile_agrees_with_our_own_maths() {
        // ⚠⚠ The load-bearing test of this module. It transforms a colour through **lcms2**, from
        // the profile we emit to lcms2's own sRGB, and compares against the result of our own
        // matrix and curve. Two independent implementations of the same transform must agree.
        //
        // This is what catches an inverted parametric tone curve — the trap here, because an ICC
        // curve describes device→linear, the inverse of what `encode` applies. Get it backwards
        // and the file opens, has the right primaries, and is washed out in a way easily mistaken
        // for a rendering preference. No other test here can see it.
        let srgb_profile = lcms2::Profile::new_srgb();
        // A mid-tone with real chroma: a neutral would pass under a wrong matrix, since every
        // sane matrix preserves grey.
        let working = color::srgb_to_prophoto([0.35, 0.55, 0.20]);

        let want_srgb = {
            let lin = color::OutputSpace::Srgb.from_working(working);
            [
                color::linear_to_srgb(lin[0]),
                color::linear_to_srgb(lin[1]),
                color::linear_to_srgb(lin[2]),
            ]
        };

        for s in color::OutputSpace::ALL {
            let ours: [f32; 3] = {
                let lin = s.from_working(working);
                [s.encode(lin[0]), s.encode(lin[1]), s.encode(lin[2])]
            };
            let profile = lcms2::Profile::new_icc(icc_for(s)).expect("our profile should parse");
            let t: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
                &profile,
                lcms2::PixelFormat::RGB_FLT,
                &srgb_profile,
                lcms2::PixelFormat::RGB_FLT,
                lcms2::Intent::RelativeColorimetric,
            )
            .expect("transform should build");
            let mut through_lcms = [[0.0f32; 3]];
            t.transform_pixels(&[ours], &mut through_lcms);

            for i in 0..3 {
                assert!(
                    (through_lcms[0][i] - want_srgb[i]).abs() < 5e-3,
                    "{}: lcms gave {} for channel {i}, our own maths gives {}",
                    s.label(),
                    through_lcms[0][i],
                    want_srgb[i]
                );
            }
        }
    }

    #[test]
    fn every_profile_names_itself() {
        // ⚠ Without this an exported file reports its profile as "RGB built-in" in every viewer,
        // so P3, Adobe RGB and ProPhoto are indistinguishable to anyone checking a file by hand.
        // The bytes differ either way — this is about the file being legible, and it was found by
        // inspecting a real export, not by a test.
        for s in color::OutputSpace::ALL {
            let p = lcms2::Profile::new_icc(icc_for(s)).expect("profile should parse");
            let desc = p
                .info(lcms2::InfoType::Description, lcms2::Locale::none())
                .unwrap_or_default();
            assert!(!desc.is_empty(), "{} has no description", s.label());
            assert!(
                desc != "RGB built-in",
                "{} kept lcms2's placeholder description",
                s.label()
            );
        }
    }
}
