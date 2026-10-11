//! The PPI a file carries (M10.10b) — what tells a print dialog how large the picture is.
//!
//! ⚠ Three containers, three places, and `image` writes only one of them: a JPEG takes it in its
//! JFIF header through the encoder ([`jpeg_density`]); a PNG gets a `pHYs` chunk and a TIFF its
//! resolution tags here, after the encode — the `metadata` module's post-pass shape. A DNG carries
//! none: it is not a print format.

use crate::ExportError;
use image::codecs::jpeg::PixelDensity;

/// A JPEG's JFIF density: whole pixels per inch, which is all the header holds.
pub fn jpeg_density(ppi: f32) -> PixelDensity {
    PixelDensity::dpi(ppi.round().clamp(1.0, f32::from(u16::MAX)) as u16)
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A PNG with a `pHYs` chunk — pixels per metre, the unit PNG has — right after `IHDR`, replacing
/// any it had.
///
/// ⚠ Before the pixels, where the specification requires `pHYs` to be.
pub fn png_with_ppi(bytes: &[u8], ppi: f32) -> Result<Vec<u8>, ExportError> {
    if bytes.len() < 8 || bytes[..8] != PNG_SIG {
        return Err(ExportError::Metadata("not a PNG".into()));
    }
    let per_metre = (f64::from(ppi) / 0.0254)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&per_metre.to_be_bytes());
    data.extend_from_slice(&per_metre.to_be_bytes());
    data.push(1);
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"pHYs");
    crc.update(&data);

    let mut out = Vec::with_capacity(bytes.len() + 21);
    out.extend_from_slice(&PNG_SIG);
    let mut at = 8;
    let mut placed = false;
    while at < bytes.len() {
        if at + 12 > bytes.len() {
            return Err(ExportError::Metadata("truncated PNG chunk".into()));
        }
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let end = at
            .checked_add(12 + len)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| ExportError::Metadata("malformed PNG chunk".into()))?;
        let kind = &bytes[at + 4..at + 8];
        if kind != b"pHYs" {
            out.extend_from_slice(&bytes[at..end]);
        }
        if kind == b"IHDR" {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(b"pHYs");
            out.extend_from_slice(&data);
            out.extend_from_slice(&crc.clone().finalize().to_be_bytes());
            placed = true;
        }
        at = end;
    }
    if !placed {
        return Err(ExportError::Metadata("PNG has no IHDR".into()));
    }
    Ok(out)
}

/// TIFF tags: XResolution, YResolution, ResolutionUnit.
const X_RESOLUTION: u16 = 282;
const Y_RESOLUTION: u16 = 283;
const RESOLUTION_UNIT: u16 = 296;

/// A TIFF whose IFD0 says `ppi` pixels per inch.
///
/// ⚠⚠ **PATCHED IN PLACE — NOT A BYTE MOVES.** The `tiff` encoder under `image` always writes
/// XResolution and YResolution as 1/1 and ResolutionUnit as none; the two rationals live at an
/// offset and the unit inside its entry, so their values are overwritten where they stand. A file
/// without all three is an error rather than a guess: it did not come from that encoder.
pub fn tiff_with_ppi(bytes: &[u8], ppi: f32) -> Result<Vec<u8>, ExportError> {
    let bad = |why: &str| ExportError::Metadata(why.to_string());
    if bytes.len() < 8 {
        return Err(bad("not a TIFF"));
    }
    let le = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(bad("not a TIFF")),
    };
    let g16 = |i: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(i..i + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let g32 = |i: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(i..i + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let put32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    if g16(2) != Some(42) {
        return Err(bad("not a classic TIFF"));
    }
    let ifd = g32(4).ok_or_else(|| bad("TIFF header is truncated"))? as usize;
    let count = g16(ifd).ok_or_else(|| bad("TIFF IFD is out of range"))? as usize;
    let mut out = bytes.to_vec();
    let numerator = (f64::from(ppi) * 100.0)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let mut found = 0;
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        let (Some(tag), Some(kind), Some(n), Some(value)) =
            (g16(e), g16(e + 2), g32(e + 4), g32(e + 8))
        else {
            return Err(bad("TIFF IFD is truncated"));
        };
        match tag {
            X_RESOLUTION | Y_RESOLUTION => {
                let at = value as usize;
                if kind != 5 || n != 1 || at + 8 > out.len() {
                    return Err(bad("TIFF resolution is not one rational"));
                }
                out[at..at + 4].copy_from_slice(&put32(numerator));
                out[at + 4..at + 8].copy_from_slice(&put32(100));
                found += 1;
            }
            RESOLUTION_UNIT => {
                if kind != 3 || n != 1 {
                    return Err(bad("TIFF resolution unit is not one short"));
                }
                // 2 = inches. A SHORT sits in the first two bytes of the value field.
                let v = if le {
                    2u16.to_le_bytes()
                } else {
                    2u16.to_be_bytes()
                };
                out[e + 8..e + 10].copy_from_slice(&v);
                found += 1;
            }
            _ => {}
        }
    }
    if found != 3 {
        return Err(bad("TIFF has no resolution tags to set"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode, ExportFormat};
    use color::OutputSpace;
    use core_image::ImageBuf;

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (x + y) as f32 / (w + h) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    /// The JFIF APP0's units and densities — read independently of the encoder.
    fn jfif(bytes: &[u8]) -> (u8, u16, u16) {
        assert_eq!(&bytes[..4], &[0xFF, 0xD8, 0xFF, 0xE0], "SOI then APP0");
        assert_eq!(&bytes[6..11], b"JFIF\0");
        (
            bytes[13],
            u16::from_be_bytes([bytes[14], bytes[15]]),
            u16::from_be_bytes([bytes[16], bytes[17]]),
        )
    }

    /// Every PNG chunk as (type, data).
    fn chunks(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 8;
        while at + 8 <= bytes.len() {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().unwrap();
            out.push((kind, bytes[at + 8..at + 8 + len].to_vec()));
            at += 12 + len;
        }
        out
    }

    /// IFD0's `tag` as raw `(type, count, value-or-offset)` — an IFD reader written apart from the
    /// writer, `metadata`'s rule.
    fn ifd0(bytes: &[u8], tag: u16) -> Option<(u16, u32, u32)> {
        let le = &bytes[..2] == b"II";
        let g16 = |i: usize| {
            let b = [bytes[i], bytes[i + 1]];
            if le {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        };
        let g32 = |i: usize| {
            let b: [u8; 4] = bytes[i..i + 4].try_into().unwrap();
            if le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let ifd = g32(4) as usize;
        (0..g16(ifd) as usize)
            .map(|i| ifd + 2 + i * 12)
            .find(|&e| g16(e) == tag)
            .map(|e| {
                let kind = g16(e + 2);
                // A SHORT's value sits in the first two bytes of the field.
                let value = if kind == 3 {
                    u32::from(g16(e + 8))
                } else {
                    g32(e + 8)
                };
                (kind, g32(e + 4), value)
            })
    }

    fn rational(bytes: &[u8], tag: u16) -> f64 {
        let (kind, count, at) = ifd0(bytes, tag).expect("the tag");
        assert_eq!((kind, count), (5, 1), "one RATIONAL");
        let le = &bytes[..2] == b"II";
        let g32 = |i: usize| {
            let b: [u8; 4] = bytes[i..i + 4].try_into().unwrap();
            if le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let at = at as usize;
        f64::from(g32(at)) / f64::from(g32(at + 4))
    }

    #[test]
    fn a_jpeg_carries_its_ppi_in_jfif() {
        let img = ramp(32, 16);
        let plain = encode(&img, ExportFormat::Jpeg, 90, OutputSpace::Srgb, None).unwrap();
        assert_ne!(
            jfif(&plain),
            (1, 300, 300),
            "the control: no PPI asked, none written"
        );
        let with = encode(&img, ExportFormat::Jpeg, 90, OutputSpace::Srgb, Some(300.0)).unwrap();
        assert_eq!(jfif(&with), (1, 300, 300));
        let lowered = encode(
            &img,
            ExportFormat::Jpeg,
            90,
            OutputSpace::Srgb,
            Some(226.42),
        )
        .unwrap();
        assert_eq!(jfif(&lowered), (1, 226, 226));
        image::load_from_memory(&with).expect("still a JPEG");
    }

    #[test]
    fn a_png_carries_its_ppi_in_phys_before_its_pixels() {
        let img = ramp(32, 16);
        for format in [ExportFormat::Png8, ExportFormat::Png16] {
            let plain = encode(&img, format, 90, OutputSpace::Srgb, None).unwrap();
            assert!(
                !chunks(&plain).iter().any(|(k, _)| k == b"pHYs"),
                "the control"
            );
            let with = encode(&img, format, 90, OutputSpace::Srgb, Some(300.0)).unwrap();
            let all = chunks(&with);
            let phys = all.iter().position(|(k, _)| k == b"pHYs").expect("pHYs");
            let idat = all.iter().position(|(k, _)| k == b"IDAT").expect("IDAT");
            assert!(phys < idat, "{format:?}: after the pixels");
            assert_eq!(all[0].0, *b"IHDR");
            let d = &all[phys].1;
            assert_eq!(d.len(), 9);
            assert_eq!(u32::from_be_bytes(d[0..4].try_into().unwrap()), 11811);
            assert_eq!(u32::from_be_bytes(d[4..8].try_into().unwrap()), 11811);
            assert_eq!(d[8], 1, "per metre");
            image::load_from_memory(&with).expect("still a PNG — its CRCs hold");
            // Written twice, it is still one chunk.
            let twice = png_with_ppi(&with, 150.0).unwrap();
            assert_eq!(
                chunks(&twice).iter().filter(|(k, _)| k == b"pHYs").count(),
                1
            );
        }
    }

    #[test]
    fn a_tiff_carries_its_ppi_in_ifd0() {
        let img = ramp(32, 16);
        for format in [ExportFormat::Tiff8, ExportFormat::Tiff16] {
            let plain = encode(&img, format, 90, OutputSpace::Srgb, None).unwrap();
            assert_eq!(rational(&plain, 282), 1.0, "the control: the encoder's 1/1");
            let with = encode(&img, format, 90, OutputSpace::Srgb, Some(300.0)).unwrap();
            assert_eq!(with.len(), plain.len(), "patched in place, nothing moved");
            assert_eq!(rational(&with, 282), 300.0);
            assert_eq!(rational(&with, 283), 300.0);
            assert_eq!(ifd0(&with, 296).map(|t| t.2), Some(2), "inches");
            let lowered = encode(&img, format, 90, OutputSpace::Srgb, Some(226.42)).unwrap();
            assert!((rational(&lowered, 282) - 226.42).abs() < 0.005);
            image::load_from_memory(&with).expect("still a TIFF");
        }
    }

    #[test]
    fn a_container_without_the_tags_is_an_error_not_a_panic() {
        assert!(png_with_ppi(b"not a png", 300.0).is_err());
        assert!(tiff_with_ppi(b"II*\0\x08\0\0\0\0\0", 300.0).is_err());
        assert!(tiff_with_ppi(b"short", 300.0).is_err());
    }
}
