//! The HDR merge's result as a floating-point DNG (M10.17a): a raw of the source camera, its data
//! the merged **normalised raw signal** — camera space, white balance undone, the reference frame's
//! exposure — in half floats, so highlights above 1.0 and the bracket's clean shadows both survive.
//!
//! Layout (M9.10's): IFD0 is an 8-bit preview carrying the DNG tags and an EXIF IFD; its SubIFD is
//! the image — `LinearRaw`, 3 × 16-bit float, 256 × 256 tiles, Deflate with the floating-point
//! predictor (TIFF Technote 3), the DNG 1.4 form Lightroom, darktable and RawTherapee read.
//!
//! ⚠⚠ The tags that make it **a raw of the camera**, each deliberate:
//! - `Make`/`Model`/`UniqueCameraModel` are the source camera's, so camera and lens profiles match.
//! - `ColorMatrix1` is libraw's `cam_xyz` at **D65** (M9.10's finding: libraw reads any colour matrix
//!   as D65), and `AsShotNeutral` the reference frame's `1 / pre_mul` — the white balance the data
//!   no longer carries, so Temp/Tint start from the light it was shot under.
//! - `Orientation` 1: libraw has already turned the frames.
//!
//! ⚠ libraw (built here without zlib) reads this file's capture facts, size and thumbnail and
//! **cannot** read its pixels; `raw_decode::float_dng` does.

use crate::tiff_ifd::{ascii, bytes, ifd_bytes, ifd_size, longs, rationals, shorts, srationals};
use crate::ExportError;
use half::f16;
use rayon::prelude::*;
use std::io::Write;
use std::path::Path;

/// The tile side, in pixels.
const TILE: usize = 256;

/// What the file is made of.
pub struct HdrDng<'a> {
    pub width: usize,
    pub height: usize,
    /// Normalised raw camera RGB, interleaved, `width * height * 3` — 1.0 is the reference frame's
    /// white; above it is what darker frames held.
    pub data: &'a [f32],
    /// The camera, as libraw names it (`"Fujifilm"`, `"X-T4"`).
    pub make: &'a str,
    pub model: &'a str,
    /// XYZ → camera, D65.
    pub cam_xyz: [[f32; 3]; 3],
    /// The camera's reading of a neutral under the reference frame's light (`1 / pre_mul`).
    pub as_shot_neutral: [f32; 3],
    /// The reference frame's capture facts, written to the EXIF IFD where known.
    pub capture: &'a core_image::Capture,
    pub lens_make: Option<&'a str>,
    /// The rendered picture, 8-bit sRGB — what a grid shows before anything decodes the file.
    pub preview: &'a image::RgbImage,
}

/// One row of half floats through TIFF Technote 3's floating-point predictor: the bytes into
/// planes, most significant first, then each byte replaced by its difference from the byte
/// `channels` before it — one **pixel** back, as libtiff and the DNG SDK both do it.
pub fn predict_row(row: &[f16], channels: usize) -> Vec<u8> {
    let n = row.len();
    let mut out = vec![0u8; n * 2];
    for (i, v) in row.iter().enumerate() {
        let [lo, hi] = v.to_bits().to_le_bytes();
        out[i] = hi;
        out[n + i] = lo;
    }
    for i in (channels..out.len()).rev() {
        out[i] = out[i].wrapping_sub(out[i - channels]);
    }
    out
}

/// A sample as stored: finite, never negative (a raw signal has no negative light — an
/// interpolating warp's undershoot is not information), and within half precision's range.
fn half(v: f32) -> f16 {
    if v.is_nan() {
        return f16::ZERO;
    }
    f16::from_f32(v.clamp(0.0, f16::MAX.to_f32()))
}

/// Exposure time as EXIF writes it: `1/n` when it is one, else to the microsecond.
fn exposure_rational(s: f32) -> (u32, u32) {
    if s > 0.0 && s < 1.0 {
        let n = (1.0 / s).round();
        if ((1.0 / n) - s).abs() / s < 0.01 {
            return (1, n as u32);
        }
        return ((s * 1e6).round() as u32, 1_000_000);
    }
    ((s * 1000.0).round() as u32, 1000)
}

fn hundredths(v: f32) -> (u32, u32) {
    ((v * 100.0).round() as u32, 100)
}

/// The EXIF IFD's entries, each only when known.
fn exif_entries(c: &core_image::Capture, lens_make: Option<&str>) -> Vec<crate::tiff_ifd::Entry> {
    let mut e = Vec::new();
    if let Some(s) = c.shutter {
        e.push(rationals(33434, &[exposure_rational(s)]));
    }
    if let Some(f) = c.aperture {
        e.push(rationals(33437, &[hundredths(f)]));
    }
    if let Some(iso) = c.iso {
        e.push(shorts(34855, &[iso.min(u16::MAX as u32) as u16]));
    }
    // ⚠ `taken` is naive wall-clock seconds (see `Capture::taken`), so it is formatted as UTC —
    // which is to say, not converted at all.
    if let Some(t) = c.taken.and_then(|t| chrono::DateTime::from_timestamp(t, 0)) {
        e.push(ascii(
            36867,
            &t.naive_utc().format("%Y:%m:%d %H:%M:%S").to_string(),
        ));
    }
    if let Some(f) = c.focal_len {
        e.push(rationals(37386, &[hundredths(f)]));
    }
    if let Some(m) = lens_make {
        e.push(ascii(42035, m));
    }
    if let Some(l) = &c.lens {
        e.push(ascii(42036, l));
    }
    e
}

/// One tile, edge-padded by repetition, through the predictor row by row, then Deflate.
fn tile(dng: &HdrDng, tx: usize, ty: usize) -> Vec<u8> {
    let (w, h) = (dng.width, dng.height);
    let (x0, y0) = (tx * TILE, ty * TILE);
    let mut raw = Vec::with_capacity(TILE * TILE * 6);
    let mut row = Vec::with_capacity(TILE * 3);
    for y in 0..TILE {
        row.clear();
        let sy = (y0 + y).min(h - 1);
        for x in 0..TILE {
            let i = (sy * w + (x0 + x).min(w - 1)) * 3;
            row.extend(dng.data[i..i + 3].iter().map(|&v| half(v)));
        }
        raw.extend(predict_row(&row, 3));
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // ⚠ Writing to a `Vec` cannot fail.
    z.write_all(&raw).expect("in memory");
    z.finish().expect("in memory")
}

/// Write `dng` at `path` — through a dot-file beside it, renamed into place, so a crash or a
/// cancelled merge never leaves half a DNG where the catalog will find it.
pub fn save(dng: &HdrDng, path: &Path) -> Result<(), ExportError> {
    let (w, h) = (dng.width, dng.height);
    let io = |e: std::io::Error| ExportError::from(image::ImageError::IoError(e));
    if w == 0 || h == 0 || dng.data.len() != w * h * 3 {
        return Err(ExportError::Metadata(format!(
            "an HDR DNG of {w}×{h} needs {} samples, got {}",
            w * h * 3,
            dng.data.len()
        )));
    }

    let (tx, ty) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let tiles: Vec<Vec<u8>> = (0..tx * ty)
        .into_par_iter()
        .map(|t| tile(dng, t % tx, t / tx))
        .collect();

    let preview = dng.preview.as_raw();
    let (pw, ph) = (dng.preview.width(), dng.preview.height());
    let flat = |m: [[f32; 3]; 3]| m.concat();
    let neutral: Vec<(u32, u32)> = dng
        .as_shot_neutral
        .iter()
        .map(|v| ((v.max(0.0) as f64 * 1e6).round() as u32, 1_000_000))
        .collect();
    let exif = exif_entries(dng.capture, dng.lens_make);
    let has_exif = !exif.is_empty();

    let raw_entries = |tile_at: &[u32]| {
        vec![
            longs(254, &[0]),
            longs(256, &[w as u32]),
            longs(257, &[h as u32]),
            shorts(258, &[16, 16, 16]),
            shorts(259, &[8]),
            shorts(262, &[34892]),
            shorts(277, &[3]),
            shorts(284, &[1]),
            shorts(317, &[3]),
            longs(322, &[TILE as u32]),
            longs(323, &[TILE as u32]),
            longs(324, tile_at),
            longs(
                325,
                &tiles.iter().map(|t| t.len() as u32).collect::<Vec<_>>(),
            ),
            shorts(339, &[3, 3, 3]),
            longs(50717, &[1]),
        ]
    };
    let ifd0_entries = |sub_at: u32, preview_at: u32, exif_at: u32| {
        let mut e = vec![
            longs(254, &[1]),
            longs(256, &[pw]),
            longs(257, &[ph]),
            shorts(258, &[8, 8, 8]),
            shorts(259, &[1]),
            shorts(262, &[2]),
            ascii(271, dng.make),
            ascii(272, dng.model),
            longs(273, &[preview_at]),
            shorts(274, &[1]),
            shorts(277, &[3]),
            longs(278, &[ph]),
            longs(279, &[preview.len() as u32]),
            shorts(284, &[1]),
            ascii(305, "Rustroom"),
            longs(330, &[sub_at]),
            bytes(50706, &[1, 4, 0, 0]),
            bytes(50707, &[1, 4, 0, 0]),
            ascii(50708, &format!("{} {}", dng.make, dng.model)),
            srationals(50721, &flat(dng.cam_xyz)),
            rationals(50728, &neutral),
            shorts(50778, &[21]), // D65 — see the module's note
        ];
        if has_exif {
            e.push(longs(34665, &[exif_at]));
        }
        e
    };

    // ⚠ Offsets depend on sizes, so the layout is fixed first: header, IFD0, EXIF, SubIFD, preview,
    // tiles. No IFD's size depends on the offsets it carries.
    let ifd0_at = 8usize;
    let exif_at = ifd0_at + ifd_size(&ifd0_entries(0, 0, 0));
    let exif_len = if has_exif { ifd_size(&exif) } else { 0 };
    let sub_at = exif_at + exif_len;
    let preview_at = sub_at + ifd_size(&raw_entries(&vec![0; tiles.len()]));
    let mut tile_at = Vec::with_capacity(tiles.len());
    let mut at = preview_at + preview.len();
    for t in &tiles {
        tile_at.push(at as u32);
        at += t.len();
    }
    if at > u32::MAX as usize {
        return Err(ExportError::Metadata(
            "the HDR DNG would pass 4 GB, past what a TIFF can address".into(),
        ));
    }

    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd0_at as u32).to_le_bytes());
    out.extend(ifd_bytes(
        ifd0_entries(sub_at as u32, preview_at as u32, exif_at as u32),
        ifd0_at,
    ));
    debug_assert_eq!(out.len(), exif_at);
    if has_exif {
        out.extend(ifd_bytes(exif, exif_at));
    }
    debug_assert_eq!(out.len(), sub_at);
    out.extend(ifd_bytes(raw_entries(&tile_at), sub_at));
    debug_assert_eq!(out.len(), preview_at);
    out.extend_from_slice(preview);
    for t in &tiles {
        out.extend_from_slice(t);
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let partial = path.with_file_name(format!(".{name}.partial"));
    std::fs::write(&partial, out).map_err(io)?;
    std::fs::rename(&partial, path).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        io(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    fn tmp(name: &str) -> std::path::PathBuf {
        test_dir::path("dng-hdr").join(name)
    }

    fn h(v: f32) -> f16 {
        f16::from_f32(v)
    }

    /// TIFF Technote 3's floating-point predictor, worked by hand: the bytes go into planes, the
    /// most significant first, then each byte becomes its difference from the byte one PIXEL
    /// (`channels` bytes) before it. 1.0 is 0x3C00 and 2.0 is 0x4000 in half precision.
    #[test]
    fn the_predictor_matches_a_hand_worked_row() {
        // One channel, two samples: planes [3C 40][00 00] → differences [3C 04 C0 00].
        assert_eq!(
            predict_row(&[h(1.0), h(2.0)], 1),
            vec![0x3C, 0x04, 0xC0, 0x00]
        );
        // Three channels, one pixel: planes [3C 40 3C][00 00 00]; each byte from the one three
        // before it → [3C 40 3C C4 C0 C4].
        assert_eq!(
            predict_row(&[h(1.0), h(2.0), h(1.0)], 3),
            vec![0x3C, 0x40, 0x3C, 0xC4, 0xC0, 0xC4]
        );
    }

    fn capture() -> core_image::Capture {
        core_image::Capture {
            // 2026-10-26 12:34:56, naive.
            taken: Some(1_793_018_096),
            camera: None,
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(400),
            shutter: Some(1.0 / 250.0),
            aperture: Some(8.0),
            focal_len: Some(23.0),
        }
    }

    fn write(path: &std::path::Path, w: usize, h: usize) -> Result<(), crate::ExportError> {
        let data: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = 0.001 + 3.0 * (i % w) as f32 / w as f32;
                [v, v * 0.8, v * 0.6]
            })
            .collect();
        let preview = image::RgbImage::from_pixel(64, 40, image::Rgb([90, 120, 150]));
        let cap = capture();
        save(
            &HdrDng {
                width: w,
                height: h,
                data: &data,
                make: "Fujifilm",
                model: "X-T4",
                cam_xyz: [
                    [1.3426, -0.6334, -0.1177],
                    [-0.4244, 1.2136, 0.2371],
                    [0.058, 0.1303, 0.598],
                ],
                as_shot_neutral: [0.516, 1.0, 0.611],
                capture: &cap,
                lens_make: Some("FUJIFILM"),
                preview: &preview,
            },
            path,
        )
    }

    /// ⚠⚠ The independent reader: libraw opens the file for everything but its pixels (it is
    /// built without zlib — probed 2026-10-26), and every capture fact must come back.
    #[test]
    fn libraw_reads_the_capture_size_and_thumbnail() {
        let path = tmp("capture.dng");
        write(&path, 300, 200).expect("writes");
        let c = raw_decode::read_capture(&path).expect("libraw opens it");
        assert_eq!(c.camera.as_deref(), Some("FUJIFILM X-T4"));
        assert_eq!(c.iso, Some(400));
        assert!((c.shutter.unwrap() - 0.004).abs() < 1e-6, "{c:?}");
        assert_eq!(c.aperture, Some(8.0));
        assert_eq!(c.focal_len, Some(23.0));
        assert_eq!(c.lens.as_deref(), Some("XF23mmF2 R WR"));
        assert_eq!(c.taken, Some(1_793_018_096));
        assert_eq!(raw_decode::dimensions(&path), Some((300, 200)));
        assert!(raw_decode::extract_thumbnail(&path).is_ok());
    }

    /// Written beside the target as a dot-file and renamed into place: nothing is left over, and a
    /// write that cannot land leaves nothing the catalog could find.
    #[test]
    fn the_partial_file_is_hidden_and_renamed() {
        let path = tmp("renamed.dng");
        write(&path, 40, 30).expect("writes");
        assert!(path.exists());
        let dir = path.parent().unwrap();
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with(".partial")),
            "a partial file is left: {names:?}"
        );
        // A target the rename cannot replace — a directory of that name.
        let blocked = tmp("blocked.dng");
        std::fs::create_dir_all(&blocked).unwrap();
        assert!(write(&blocked, 40, 30).is_err());
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with(".partial")),
            "a failed write left its partial file: {names:?}"
        );
    }
}
