//! The import survey's fast path (M9.12): a photo's capture facts and its EXIF thumbnail from the
//! first few tens of kilobytes, never the whole file.
//!
//! ⚠⚠ **Why this exists: the author's first import was "really slow" over hundreds of files.** The
//! survey read every RAW whole (26.6 MB for an X-T4 RAF) through libraw — and libraw cannot open a
//! truncated file (measured: nothing at 1 MB, half at 4 MB), so a prefix read through it is not an
//! option. But a camera JPEG, and the JPEG a Fuji RAF embeds, both OPEN with an EXIF block carrying
//! the date, the camera and a 160×120 thumbnail: on the author's own files the RAF's embedded JPEG
//! starts at byte 148 and its EXIF block is 65,448 bytes, so ~66 KB is read instead of 26.6 MB.
//!
//! ⚠ Formats it does not understand return `Ok(None)` and the caller falls back to the full read,
//! so this can only ever make the survey faster, never make a file disappear.

use core_image::{Capture, Thumbnail};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What the fast path found.
#[derive(Debug)]
pub struct QuickLook {
    pub capture: Capture,
    /// The EXIF thumbnail, orientation applied; `None` when the block carries none.
    pub thumb: Option<Thumbnail>,
}

/// A RAF's magic, and where its header records the embedded JPEG's offset (big-endian `u32`).
const RAF_MAGIC: &[u8; 16] = b"FUJIFILMCCD-RAW ";
const RAF_JPEG_OFFSET_AT: u64 = 0x54;
/// Segments walked before giving up on finding the EXIF block — it is first in every camera file.
const MAX_SEGMENTS: usize = 16;

/// Capture facts and the EXIF thumbnail of a JPEG or a Fuji RAF, reading only its EXIF block.
/// `Ok(None)` for any other format, or a JPEG with no EXIF block.
pub fn quick_look(path: &Path) -> std::io::Result<Option<QuickLook>> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 16];
    if f.read(&mut magic)? < 2 {
        return Ok(None);
    }
    let jpeg_start = if &magic == RAF_MAGIC {
        f.seek(SeekFrom::Start(RAF_JPEG_OFFSET_AT))?;
        let mut off = [0u8; 4];
        f.read_exact(&mut off)?;
        u64::from(u32::from_be_bytes(off))
    } else if magic[..2] == [0xFF, 0xD8] {
        0
    } else {
        return Ok(None);
    };
    let Some(block) = exif_block(&mut f, jpeg_start)? else {
        return Ok(None);
    };
    let Ok(ex) = exif::Reader::new().read_raw(block) else {
        return Ok(None);
    };
    Ok(Some(QuickLook {
        capture: crate::capture_of(&ex),
        thumb: thumbnail_of(&ex),
    }))
}

/// The TIFF body of the `APP1 Exif` segment of the JPEG at `start`, reading segment headers only.
fn exif_block(f: &mut std::fs::File, start: u64) -> std::io::Result<Option<Vec<u8>>> {
    f.seek(SeekFrom::Start(start))?;
    let mut soi = [0u8; 2];
    f.read_exact(&mut soi)?;
    if soi != [0xFF, 0xD8] {
        return Ok(None);
    }
    for _ in 0..MAX_SEGMENTS {
        let mut head = [0u8; 4];
        if f.read_exact(&mut head).is_err() || head[0] != 0xFF {
            return Ok(None);
        }
        let marker = head[1];
        // Start of scan: the image data begins and there are no more headers.
        if marker == 0xDA {
            return Ok(None);
        }
        let len = u16::from_be_bytes([head[2], head[3]]) as usize;
        let body = len.saturating_sub(2);
        if marker == 0xE1 {
            let mut buf = vec![0u8; body];
            f.read_exact(&mut buf)?;
            if buf.starts_with(b"Exif\0\0") {
                buf.drain(..6);
                return Ok(Some(buf));
            }
        } else {
            f.seek(SeekFrom::Current(body as i64))?;
        }
    }
    Ok(None)
}

/// The IFD1 thumbnail, decoded, with the photo's own orientation applied.
fn thumbnail_of(ex: &exif::Exif) -> Option<Thumbnail> {
    let uint = |tag, ifd| ex.get_field(tag, ifd).and_then(|f| f.value.get_uint(0));
    let off = uint(exif::Tag::JPEGInterchangeFormat, exif::In::THUMBNAIL)? as usize;
    let len = uint(exif::Tag::JPEGInterchangeFormatLength, exif::In::THUMBNAIL)? as usize;
    let bytes = ex.buf().get(off..off.checked_add(len)?)?;
    let mut img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?;
    if let Some(o) = uint(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|o| image::metadata::Orientation::from_exif(o as u8))
    {
        img.apply_orientation(o);
    }
    let rgba = img.to_rgba8();
    Some(Thumbnail {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use exif::experimental::Writer;
    use exif::{Field, In, Tag, Value};

    fn temp_file(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustroom_quicklook_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("f")
    }

    /// A 16×8 JPEG — the thumbnail the EXIF block will carry.
    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(16, 8, image::Rgb([200, 40, 40]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    /// A camera-shaped JPEG: SOI, an APP0 first (as some cameras write), `APP1 Exif` with a date,
    /// a make and model, orientation 6 and a thumbnail, then a stub scan. Constructed, not captured
    /// — the captured shape was checked by hand on the author's X-T4 files (see the module doc).
    fn camera_jpeg(orientation: u16) -> Vec<u8> {
        let thumb = tiny_jpeg();
        let date = Field {
            tag: Tag::DateTimeOriginal,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"2026:09:27 12:00:00".to_vec()]),
        };
        let make = Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"FUJIFILM".to_vec()]),
        };
        let model = Field {
            tag: Tag::Model,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"X-T4".to_vec()]),
        };
        let orient = Field {
            tag: Tag::Orientation,
            ifd_num: In::PRIMARY,
            value: Value::Short(vec![orientation]),
        };
        let mut w = Writer::new();
        for f in [&date, &make, &model, &orient] {
            w.push_field(f);
        }
        w.set_jpeg(&thumb, In::THUMBNAIL);
        let mut tiff = std::io::Cursor::new(Vec::new());
        w.write(&mut tiff, false).unwrap();
        let tiff = tiff.into_inner();

        let mut out = vec![0xFF, 0xD8];
        out.extend([0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]); // an APP0 to skip
        let len = (tiff.len() + 6 + 2) as u16;
        out.extend([0xFF, 0xE1]);
        out.extend(len.to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(&tiff);
        out.extend([0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        out
    }

    /// A RAF-shaped wrapper: the magic, the JPEG's offset at 0x54, padding, the JPEG, then a
    /// megabyte of "sensor data" that the fast path must never need.
    fn raf(jpeg: &[u8]) -> Vec<u8> {
        let mut out = RAF_MAGIC.to_vec();
        out.resize(RAF_JPEG_OFFSET_AT as usize, 0);
        let at = 148u32;
        out.extend(at.to_be_bytes());
        out.extend((jpeg.len() as u32).to_be_bytes());
        out.resize(at as usize, 0);
        out.extend(jpeg);
        out.extend(vec![0x5A; 1 << 20]);
        out
    }

    #[test]
    fn a_camera_jpeg_gives_its_date_camera_and_thumbnail() {
        let p = temp_file("jpeg");
        std::fs::write(&p, camera_jpeg(1)).unwrap();
        let q = quick_look(&p).unwrap().expect("a JPEG with EXIF");
        // 2026-09-27 12:00:00, naive wall-clock seconds
        assert_eq!(q.capture.taken, Some(1_790_510_400));
        assert_eq!(q.capture.camera.as_deref(), Some("FUJIFILM X-T4"));
        let t = q.thumb.expect("the EXIF thumbnail");
        assert_eq!((t.width, t.height), (16, 8));
    }

    #[test]
    fn a_raf_is_read_through_its_embedded_jpeg_and_orientation_is_applied() {
        let p = temp_file("raf");
        std::fs::write(&p, raf(&camera_jpeg(6))).unwrap();
        let q = quick_look(&p)
            .unwrap()
            .expect("a RAF with an embedded JPEG");
        assert_eq!(q.capture.taken, Some(1_790_510_400));
        let t = q.thumb.expect("the thumbnail");
        assert_eq!(
            (t.width, t.height),
            (8, 16),
            "orientation 6 turns it upright"
        );
    }

    #[test]
    fn anything_else_is_declined_so_the_caller_falls_back() {
        let p = temp_file("other");
        std::fs::write(&p, b"II*\0 a TIFF-based raw").unwrap();
        assert!(quick_look(&p).unwrap().is_none());
        // A JPEG with no EXIF block at all.
        std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]).unwrap();
        assert!(quick_look(&p).unwrap().is_none());
        // A truncated RAF must not panic.
        std::fs::write(&p, &RAF_MAGIC[..]).unwrap();
        assert!(quick_look(&p).is_err() || quick_look(&p).unwrap().is_none());
    }
}
