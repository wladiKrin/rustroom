//! Floating-point linear DNGs, read by Rustroom itself (M10.17a) — the HDR merge's result.
//!
//! ⚠⚠ **Why not libraw.** The libraw `rsraw` bundles is built without zlib: on 2026-10-26 it opened a
//! Deflate-compressed float DNG for its capture facts, size and thumbnail — all correct — and failed
//! to unpack the pixels; an uncompressed one it read but **clipped to ~1** (3.0 came back 1.07),
//! converting floats to 16-bit integers. An HDR is exactly the values above 1. So the pixels are
//! read here and libraw keeps everything else it reads correctly.
//!
//! What is read: a little-endian TIFF whose IFD0 or one of its SubIFDs is a full-resolution
//! `LinearRaw` image of 3 float samples (16- or 32-bit), tiled or striped, uncompressed or Deflate,
//! with no predictor or the floating-point one. Anything else goes to libraw as before.
//!
//! The pixels then take **exactly** the path a raw's take in `backend_rsraw::decode`: the as-shot
//! white balance (`1 / AsShotNeutral`, scaled so the smallest is 1, as libraw scales `pre_mul`), then
//! `camera_to_prophoto(rgb_cam)` with dcraw's `rgb_cam` from `ColorMatrix1` — or the `.dcp`'s
//! transform when one is chosen. Values are never clamped.

use crate::{DecodeError, DecodeOptions, DecodedRaw, RawMetadata};
use core_image::ImageBuf;
use half::f16;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;

/// One IFD entry as read: its type, count and the bytes of its value.
struct Field {
    kind: u16,
    count: usize,
    bytes: Vec<u8>,
}

type Ifd = HashMap<u16, Field>;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The size of one value of a TIFF field type, for the types this reader meets.
fn type_size(kind: u16) -> Option<usize> {
    match kind {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn read_ifd(b: &[u8], at: usize) -> Option<Ifd> {
    let n = u16_at(b, at)? as usize;
    let mut ifd = HashMap::new();
    for i in 0..n {
        let e = at + 2 + 12 * i;
        let (tag, kind, count) = (u16_at(b, e)?, u16_at(b, e + 2)?, u32_at(b, e + 4)? as usize);
        let Some(size) = type_size(kind) else {
            continue;
        };
        let len = size.checked_mul(count)?;
        let bytes = if len <= 4 {
            b.get(e + 8..e + 8 + len)?.to_vec()
        } else {
            let off = u32_at(b, e + 8)? as usize;
            b.get(off..off.checked_add(len)?)?.to_vec()
        };
        ifd.insert(tag, Field { kind, count, bytes });
    }
    Some(ifd)
}

/// A SHORT or LONG field as integers.
fn ints(ifd: &Ifd, tag: u16) -> Option<Vec<u32>> {
    let f = ifd.get(&tag)?;
    match f.kind {
        3 => Some(
            (0..f.count)
                .filter_map(|i| u16_at(&f.bytes, 2 * i).map(u32::from))
                .collect(),
        ),
        4 => Some(
            (0..f.count)
                .filter_map(|i| u32_at(&f.bytes, 4 * i))
                .collect(),
        ),
        _ => None,
    }
}

fn int(ifd: &Ifd, tag: u16) -> Option<u32> {
    ints(ifd, tag)?.first().copied()
}

/// A RATIONAL or SRATIONAL field as reals.
fn reals(ifd: &Ifd, tag: u16) -> Option<Vec<f64>> {
    let f = ifd.get(&tag)?;
    let pair = |i: usize| -> Option<f64> {
        let (n, d) = (u32_at(&f.bytes, 8 * i)?, u32_at(&f.bytes, 8 * i + 4)?);
        let (n, d) = if f.kind == 10 {
            (n as i32 as f64, d as i32 as f64)
        } else {
            (n as f64, d as f64)
        };
        (d != 0.0).then_some(n / d)
    };
    match f.kind {
        5 | 10 => (0..f.count).map(pair).collect(),
        _ => None,
    }
}

/// Where a float `LinearRaw` image lives and how it is stored.
struct Layout {
    width: usize,
    height: usize,
    bits: u32,
    compression: u32,
    predictor: u32,
    /// `(x, y, w, h, offset, length)` per tile or strip.
    blocks: Vec<(usize, usize, usize, usize, usize, usize)>,
}

/// IFD0, and the image IFD if this is a float linear DNG.
fn find(b: &[u8]) -> Option<(Ifd, Layout)> {
    if b.get(0..4)? != b"II*\0" {
        return None;
    }
    let ifd0 = read_ifd(b, u32_at(b, 4)? as usize)?;
    ifd0.get(&50706)?; // DNGVersion: a DNG at all
    let mut candidates = vec![];
    for at in ints(&ifd0, 330).unwrap_or_default() {
        if let Some(i) = read_ifd(b, at as usize) {
            candidates.push(i);
        }
    }
    let image = candidates
        .into_iter()
        .chain(std::iter::once(read_ifd(b, u32_at(b, 4)? as usize)?))
        .find(|i| {
            int(i, 254).unwrap_or(0) == 0
                && int(i, 262) == Some(34892)
                && int(i, 277) == Some(3)
                && ints(i, 339).is_some_and(|f| !f.is_empty() && f.iter().all(|v| *v == 3))
        })?;
    let (width, height) = (int(&image, 256)? as usize, int(&image, 257)? as usize);
    let bits = int(&image, 258)?;
    let compression = int(&image, 259).unwrap_or(1);
    let predictor = int(&image, 317).unwrap_or(1);
    if !matches!(bits, 16 | 32) || !matches!(compression, 1 | 8) || !matches!(predictor, 1 | 3) {
        return None;
    }
    let mut blocks = vec![];
    if let (Some(tw), Some(th)) = (int(&image, 322), int(&image, 323)) {
        let (tw, th) = (tw as usize, th as usize);
        let (offs, lens) = (ints(&image, 324)?, ints(&image, 325)?);
        let across = width.div_ceil(tw);
        for (i, (o, l)) in offs.iter().zip(&lens).enumerate() {
            blocks.push((
                (i % across) * tw,
                (i / across) * th,
                tw,
                th,
                *o as usize,
                *l as usize,
            ));
        }
    } else {
        let rows = int(&image, 278).map_or(height, |r| r as usize);
        let (offs, lens) = (ints(&image, 273)?, ints(&image, 279)?);
        for (i, (o, l)) in offs.iter().zip(&lens).enumerate() {
            blocks.push((0, i * rows, width, rows, *o as usize, *l as usize));
        }
    }
    (width > 0 && height > 0 && !blocks.is_empty()).then_some((
        ifd0,
        Layout {
            width,
            height,
            bits,
            compression,
            predictor,
            blocks,
        },
    ))
}

/// Whether `bytes` is a DNG this module reads.
pub(crate) fn is_float_linear(bytes: &[u8]) -> bool {
    find(bytes).is_some()
}

/// Undo [`export::dng_hdr::predict_row`]'s floating-point predictor on one row of `bytes`
/// (`samples × bytes_per_sample` long) in place, returning the row's values.
pub(crate) fn unpredict_row(
    bytes: &mut [u8],
    channels: usize,
    bytes_per_sample: usize,
) -> Vec<f32> {
    for i in channels..bytes.len() {
        bytes[i] = bytes[i].wrapping_add(bytes[i - channels]);
    }
    let n = bytes.len() / bytes_per_sample;
    (0..n)
        .map(|i| {
            // Plane 0 holds each sample's most significant byte.
            let byte = |plane: usize| bytes[plane * n + i];
            match bytes_per_sample {
                2 => f16::from_bits(u16::from_be_bytes([byte(0), byte(1)])).to_f32(),
                _ => f32::from_bits(u32::from_be_bytes([byte(0), byte(1), byte(2), byte(3)])),
            }
        })
        .collect()
}

/// One row of samples stored without a predictor.
fn plain_row(bytes: &[u8], bytes_per_sample: usize) -> Vec<f32> {
    bytes
        .chunks_exact(bytes_per_sample)
        .map(|c| match bytes_per_sample {
            2 => f16::from_bits(u16::from_le_bytes([c[0], c[1]])).to_f32(),
            _ => f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
        })
        .collect()
}

/// The image's samples, `width * height * 3`, as stored.
fn samples(b: &[u8], l: &Layout) -> Result<Vec<f32>, DecodeError> {
    let bps = (l.bits / 8) as usize;
    let decoded: Vec<Result<(usize, usize, usize, usize, Vec<f32>), DecodeError>> = l
        .blocks
        .par_iter()
        .map(|&(x0, y0, bw, bh, off, len)| {
            let src = b
                .get(off..off + len)
                .ok_or_else(|| DecodeError::Format("a DNG tile runs past the file".into()))?;
            let raw = if l.compression == 8 {
                let mut out = Vec::with_capacity(bw * bh * 3 * bps);
                flate2::read::ZlibDecoder::new(src)
                    .read_to_end(&mut out)
                    .map_err(|e| DecodeError::Format(format!("inflating a DNG tile: {e}")))?;
                out
            } else {
                src.to_vec()
            };
            let row_bytes = bw * 3 * bps;
            if raw.len() < row_bytes * bh.min(l.height - y0) {
                return Err(DecodeError::Format("a DNG tile is short".into()));
            }
            let mut vals = Vec::with_capacity(bw * bh * 3);
            for row in raw.chunks_exact(row_bytes).take(bh) {
                if l.predictor == 3 {
                    vals.extend(unpredict_row(&mut row.to_vec(), 3, bps));
                } else {
                    vals.extend(plain_row(row, bps));
                }
            }
            Ok((x0, y0, bw, bh, vals))
        })
        .collect();
    let mut out = vec![0f32; l.width * l.height * 3];
    for block in decoded {
        let (x0, y0, bw, bh, vals) = block?;
        for y in 0..bh {
            let gy = y0 + y;
            if gy >= l.height {
                break;
            }
            for x in 0..bw.min(l.width.saturating_sub(x0)) {
                let s = (y * bw + x) * 3;
                if s + 3 > vals.len() {
                    break;
                }
                let d = (gy * l.width + x0 + x) * 3;
                out[d..d + 3].copy_from_slice(&vals[s..s + 3]);
            }
        }
    }
    Ok(out)
}

/// libraw's flip code applied to a decoded image (dcraw codes: 3 = 180°, 5 = 90° CCW, 6 = 90° CW).
fn oriented(img: ImageBuf, flip: i32) -> ImageBuf {
    let (w, h) = (img.width(), img.height());
    let (ow, oh) = if matches!(flip, 5 | 6) {
        (h, w)
    } else {
        (w, h)
    };
    if !matches!(flip, 3 | 5 | 6) {
        return img;
    }
    let mut out = ImageBuf::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match flip {
                3 => (w - 1 - x, h - 1 - y),
                5 => (w - 1 - y, x),
                _ => (y, h - 1 - x),
            };
            out.set_pixel(x, y, img.pixel(sx, sy));
        }
    }
    out
}

/// Decode a float linear DNG — the counterpart of `backend_rsraw::decode`.
pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let (ifd0, l) = find(bytes).ok_or_else(|| DecodeError::Format("not a float DNG".into()))?;
    let matrix = |tag: u16| -> Option<[[f32; 3]; 3]> {
        let v = reals(&ifd0, tag)?;
        (v.len() == 9).then(|| {
            [
                [v[0] as f32, v[1] as f32, v[2] as f32],
                [v[3] as f32, v[4] as f32, v[5] as f32],
                [v[6] as f32, v[7] as f32, v[8] as f32],
            ]
        })
    };
    let cam_xyz = matrix(50721)
        .ok_or_else(|| DecodeError::Format("a float DNG with no ColorMatrix1".into()))?;
    let neutral = reals(&ifd0, 50728)
        .filter(|n| n.len() == 3 && n.iter().all(|v| *v > 0.0))
        .map_or([1.0f32; 3], |n| [n[0] as f32, n[1] as f32, n[2] as f32]);
    let cam_mul = {
        let m = neutral.map(|n| 1.0 / n);
        let min = m.iter().cloned().fold(f32::INFINITY, f32::min);
        m.map(|v| v / min)
    };
    let rgb_cam = color::camera::rgb_cam_from_cam_xyz(&cam_xyz)
        .ok_or_else(|| DecodeError::Format("a float DNG's colour matrix is singular".into()))?;

    let data = samples(bytes, &l)?;
    let full = ImageBuf::from_vec(l.width, l.height, data);
    let img = if opts.half_size {
        let (hw, hh) = (l.width / 2, l.height / 2);
        let mut half = ImageBuf::new(hw.max(1), hh.max(1));
        for y in 0..hh {
            for x in 0..hw {
                let mut s = [0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = full.pixel(2 * x + dx, 2 * y + dy);
                    for c in 0..3 {
                        s[c] += p[c];
                    }
                }
                half.set_pixel(x, y, s.map(|v| v * 0.25));
            }
        }
        half
    } else {
        full
    };

    let (m, hue_sat) = match opts
        .camera_profile
        .as_deref()
        .and_then(|p| crate::backend_rsraw::profile_transform(p, cam_mul))
    {
        Some(t) => (t.matrix, t.hue_sat),
        None => (color::camera::camera_to_prophoto(rgb_cam), None),
    };
    let image = img.map_pixels(|p| {
        let p = color::spaces::apply(
            &m,
            [p[0] * cam_mul[0], p[1] * cam_mul[1], p[2] * cam_mul[2]],
        );
        match &hue_sat {
            Some(t) => t.apply(p),
            None => p,
        }
    });

    // ⚠ The metadata is libraw's: it opens these files and reads them correctly.
    let raw = rsraw::RawImage::open(bytes).ok();
    let flip = raw.as_ref().map_or(0, |r| r.as_ref().sizes.flip);
    let image = oriented(image, flip);
    let (width, height) = (image.width(), image.height());
    let (make, model, lens) = match &raw {
        Some(r) => {
            let li = r.lens_info();
            let name = li.lens_name.trim().to_string();
            (
                r.make().to_string(),
                r.model().to_string(),
                (!name.is_empty()).then(|| crate::LensMetadata {
                    make: li.lens_make.trim().to_string(),
                    name,
                    focal: r.focal_len(),
                    aperture: r.aperture(),
                }),
            )
        }
        None => (String::new(), String::new(), None),
    };
    Ok(DecodedRaw {
        image,
        metadata: RawMetadata {
            width,
            height,
            make,
            model,
            lens,
            as_shot: crate::backend_rsraw::as_shot_illuminant(&cam_xyz, cam_mul),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_raw, DecodeOptions};
    use half::f16;

    const CAM_XYZ: [[f32; 3]; 3] = [
        [1.3426, -0.6334, -0.1177],
        [-0.4244, 1.2136, 0.2371],
        [0.058, 0.1303, 0.598],
    ];
    const NEUTRAL: [f32; 3] = [0.516, 1.0, 0.611];

    fn tmp(name: &str) -> std::path::PathBuf {
        test_dir::path("float-dng").join(name)
    }

    /// Normalised raw data with deep shadows, mid-tones and highlights far above 1.
    fn data(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let v = 0.0004 * 2f32.powf(13.5 * x as f32 / w as f32);
                [v, v * (0.5 + y as f32 / h as f32), v * 0.7]
            })
            .collect()
    }

    fn write(path: &std::path::Path, w: usize, h: usize, d: &[f32]) {
        let cap = core_image::Capture::default();
        let preview = image::RgbImage::from_pixel(32, 24, image::Rgb([128, 128, 128]));
        export::dng_hdr::save(
            &export::dng_hdr::HdrDng {
                width: w,
                height: h,
                data: d,
                make: "Fujifilm",
                model: "X-T4",
                cam_xyz: CAM_XYZ,
                as_shot_neutral: NEUTRAL,
                capture: &cap,
                lens_make: None,
                preview: &preview,
            },
            path,
        )
        .expect("writes");
    }

    /// The multipliers the reader must apply: `1 / AsShotNeutral`, scaled so the smallest is 1 — the
    /// way libraw scales `pre_mul`.
    fn mul() -> [f32; 3] {
        let m = NEUTRAL.map(|n| 1.0 / n);
        let min = m.iter().cloned().fold(f32::INFINITY, f32::min);
        m.map(|v| v / min)
    }

    /// ⚠⚠ The data back through exactly the transform a raw gets: the white balance, then
    /// `camera_to_prophoto(rgb_cam)` with dcraw's `rgb_cam` from the colour matrix — and **nothing
    /// clamped**, which is the reason this reader exists (libraw read 3.0 as 1.07, 2026-10-26).
    #[test]
    fn a_float_dng_reads_back_as_its_data_through_the_camera_matrix() {
        let (w, h) = (300, 200);
        let d = data(w, h);
        let path = tmp("roundtrip.dng");
        write(&path, w, h, &d);
        let got = decode_raw(&path, &DecodeOptions::linear_scene()).expect("reads");
        assert_eq!((got.image.width(), got.image.height()), (w, h));
        let m = color::camera::camera_to_prophoto(
            color::camera::rgb_cam_from_cam_xyz(&CAM_XYZ).unwrap(),
        );
        let k = mul();
        let mut worst = 0f32;
        let mut brightest = 0f32;
        for (src, out) in d.chunks(3).zip(got.image.data().chunks(3)) {
            assert!(out.iter().all(|v| v.is_finite()), "non-finite {out:?}");
            let bal = [src[0] * k[0], src[1] * k[1], src[2] * k[2]];
            let want = color::spaces::apply(&m, bal);
            let scale = bal.iter().cloned().fold(0f32, f32::max);
            for c in 0..3 {
                worst = worst.max((out[c] - want[c]).abs() / scale);
                brightest = brightest.max(out[c]);
            }
        }
        eprintln!("float DNG round trip: worst relative {worst}, brightest {brightest}");
        assert!(worst < 2e-3, "worst relative error {worst}");
        assert!(brightest > 3.0, "the highlights were clamped: {brightest}");
    }

    /// One tile wider than the whole image — the shape libraw misreads in M9.10's DNGs — reads back.
    #[test]
    fn an_image_narrower_than_a_tile_reads_back() {
        let (w, h) = (100, 60);
        let d = data(w, h);
        let path = tmp("narrow.dng");
        write(&path, w, h, &d);
        let got = decode_raw(&path, &DecodeOptions::linear_scene()).expect("reads");
        let m = color::camera::camera_to_prophoto(
            color::camera::rgb_cam_from_cam_xyz(&CAM_XYZ).unwrap(),
        );
        let k = mul();
        let (src, out) = (&d[(37 * w + 81) * 3..], got.image.pixel(81, 37));
        let want = color::spaces::apply(&m, [src[0] * k[0], src[1] * k[1], src[2] * k[2]]);
        for c in 0..3 {
            assert!(
                (out[c] - want[c]).abs() <= 2e-3 * want[c].abs().max(1e-3),
                "{out:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn half_size_averages_two_by_two() {
        let (w, h) = (300, 200);
        let d = data(w, h);
        let path = tmp("half.dng");
        write(&path, w, h, &d);
        let full = decode_raw(&path, &DecodeOptions::linear_scene())
            .unwrap()
            .image;
        let half = decode_raw(&path, &DecodeOptions::preview()).unwrap().image;
        assert_eq!((half.width(), half.height()), (w / 2, h / 2));
        for (x, y) in [(0, 0), (37, 21), (149, 99)] {
            let mut avg = [0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = full.pixel(2 * x + dx, 2 * y + dy);
                for c in 0..3 {
                    avg[c] += p[c] / 4.0;
                }
            }
            let got = half.pixel(x, y);
            for c in 0..3 {
                assert!(
                    (got[c] - avg[c]).abs() <= 1e-5 * avg[c].abs().max(1.0),
                    "({x}, {y}): {got:?} vs {avg:?}"
                );
            }
        }
    }

    /// M9.10's rendered DNG is 16-bit integer lossless JPEG — libraw's, as before.
    #[test]
    fn a_rendered_m9_10_dng_still_goes_to_libraw() {
        let path = tmp("rendered.dng");
        let mut img = core_image::ImageBuf::new(40, 30);
        img.set_pixel(0, 0, [1.0, 1.0, 1.0]);
        export::dng::save(&img, &path).unwrap();
        assert!(!is_float_linear(&std::fs::read(&path).unwrap()));
        assert!(!crate::is_float_dng(&path));
        assert!(decode_raw(&path, &DecodeOptions::linear_scene()).is_ok());
    }

    /// The as-shot light comes from the file's own neutral and matrix, as a raw's does.
    #[test]
    fn the_float_reader_reports_the_as_shot_light() {
        let path = tmp("as-shot.dng");
        write(&path, 64, 48, &data(64, 48));
        assert!(crate::is_float_dng(&path));
        let got = decode_raw(&path, &DecodeOptions::preview()).unwrap();
        assert_eq!(
            got.metadata.as_shot,
            crate::backend_rsraw::as_shot_illuminant(&CAM_XYZ, mul())
        );
        assert!(got.metadata.as_shot.is_some());
        assert_eq!(got.metadata.make, "Fujifilm");
        assert_eq!(got.metadata.model, "X-T4");
    }

    #[test]
    fn the_predictor_round_trips() {
        let row: Vec<f16> = (0..30)
            .map(|i| f16::from_f32(0.001 * (i * i) as f32 + 0.3 * (i % 3) as f32))
            .collect();
        let mut bytes = export::dng_hdr::predict_row(&row, 3);
        let back = unpredict_row(&mut bytes, 3, 2);
        let want: Vec<f32> = row.iter().map(|v| v.to_f32()).collect();
        assert_eq!(back, want);
    }
}
