//! The finished picture as a linear DNG (M9.10): Rustroom's render, every edit baked in, in a file
//! other raw editors open as a raw.
//!
//! Layout: IFD0 is an 8-bit preview (≤256 px) carrying the DNG tags; its SubIFD is the image —
//! `LinearRaw`, 3 × 16-bit linear ProPhoto, lossless-JPEG tiles (`lj92`).
//!
//! ⚠⚠ **The tags that keep the picture "intact" in a Lightroom-style reader**, each deliberate:
//! - `ColorMatrix1` (at D65) / `ForwardMatrix1` (to D50) describe linear ProPhoto, and `AsShotNeutral` is
//!   1,1,1 — the data is already white-balanced, so the reader converts colour and changes nothing.
//! - `ProfileToneCurve` is the identity and `BaselineExposure` 0: no contrast or brightness of the
//!   reader's own on top of Rustroom's.
//! - `DefaultBlackRender` "none": no automatic black-point adjustment.
//! - `UniqueCameraModel` is "Rustroom Rendered", deliberately NOT the source camera: a reader that
//!   recognised the X-T4 would apply the X-T4's profile to data that is no longer X-T4 colour.
//!
//! ⚠ darktable's scene-referred workflow ignores the profile tone curve and adds its own exposure
//! and tone mapping to anything raw — measured, and reported in the journal, not claimed away.

use crate::ExportError;
use core_image::ImageBuf;
use std::path::Path;

/// The tile side, in pixels.
const TILE: usize = 256;
/// The preview's long edge, in pixels.
const PREVIEW: usize = 256;

// TIFF field types.
const BYTE: u16 = 1;
const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;
const SRATIONAL: u16 = 10;
const FLOAT: u16 = 11;

/// One IFD entry: tag, type, count and the value's bytes (little-endian).
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    bytes: Vec<u8>,
}

fn ascii(tag: u16, s: &str) -> Entry {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: bytes.len() as u32,
        bytes,
    }
}

fn shorts(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

fn longs(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        kind: BYTE,
        count: v.len() as u32,
        bytes: v.to_vec(),
    }
}

fn floats(tag: u16, v: &[f32]) -> Entry {
    Entry {
        tag,
        kind: FLOAT,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

/// Signed rationals with a fixed denominator of 10⁶ — six decimals is past a colour matrix's own
/// precision.
fn srationals(tag: u16, v: &[f32]) -> Entry {
    Entry {
        tag,
        kind: SRATIONAL,
        count: v.len() as u32,
        bytes: v
            .iter()
            .flat_map(|x| {
                let n = (*x as f64 * 1e6).round() as i32;
                [n.to_le_bytes(), 1_000_000i32.to_le_bytes()].concat()
            })
            .collect(),
    }
}

fn rationals(tag: u16, v: &[(u32, u32)]) -> Entry {
    Entry {
        tag,
        kind: RATIONAL,
        count: v.len() as u32,
        bytes: v
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
            .collect(),
    }
}

/// The size an IFD with `entries` occupies, its out-of-line values included.
fn ifd_size(entries: &[Entry]) -> usize {
    2 + 12 * entries.len()
        + 4
        + entries
            .iter()
            .filter(|e| e.bytes.len() > 4)
            .map(|e| e.bytes.len().div_ceil(2) * 2)
            .sum::<usize>()
}

/// Serialise an IFD placed at `at`, its out-of-line values right after it. Sorted by tag, as TIFF
/// requires.
fn ifd_bytes(mut entries: Vec<Entry>, at: usize) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let data_at = at + 2 + 12 * entries.len() + 4;
    head.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in &entries {
        head.extend_from_slice(&e.tag.to_le_bytes());
        head.extend_from_slice(&e.kind.to_le_bytes());
        head.extend_from_slice(&e.count.to_le_bytes());
        if e.bytes.len() <= 4 {
            let mut v = e.bytes.clone();
            v.resize(4, 0);
            head.extend_from_slice(&v);
        } else {
            head.extend_from_slice(&((data_at + tail.len()) as u32).to_le_bytes());
            tail.extend_from_slice(&e.bytes);
            if tail.len() % 2 == 1 {
                tail.push(0); // word-align, as TIFF asks
            }
        }
    }
    head.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    head.extend_from_slice(&tail);
    head
}

/// 3×3 inverse.
fn inverse(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let m = m.map(|r| r.map(|v| v as f64));
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
    };
    let mut out = [[0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (c(j, i) / det) as f32;
        }
    }
    out
}

/// Write `linear` — the render in **linear ProPhoto**, display-referred, nominally `[0, 1]` — as a
/// linear DNG at `path`.
pub fn save(linear: &ImageBuf, path: &Path) -> Result<(), ExportError> {
    let (w, h) = (linear.width(), linear.height());
    let io = |e: std::io::Error| ExportError::from(image::ImageError::IoError(e));

    // The image, as 256×256 tiles, each one lossless JPEG. An edge tile is padded by repeating the
    // last column and row, which costs almost nothing to compress.
    let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let (tx, ty) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let mut tiles: Vec<Vec<u8>> = Vec::with_capacity(tx * ty);
    for t in 0..tx * ty {
        let (x0, y0) = ((t % tx) * TILE, (t / tx) * TILE);
        let mut s = Vec::with_capacity(TILE * TILE * 3);
        for y in 0..TILE {
            for x in 0..TILE {
                let p = linear.pixel((x0 + x).min(w - 1), (y0 + y).min(h - 1));
                s.extend(p.map(q));
            }
        }
        tiles.push(crate::lj92::encode(&s, TILE, TILE, 3));
    }

    // The preview: ≤256 px, sRGB, 8-bit, uncompressed.
    let small = linear.downscale_to(PREVIEW);
    let (pw, ph) = (small.width(), small.height());
    let preview: Vec<u8> = small
        .data()
        .chunks(3)
        .flat_map(|c| {
            let lin = color::OutputSpace::Srgb.from_working([c[0], c[1], c[2]]);
            lin.map(|v| (color::OutputSpace::Srgb.encode(v) * 255.0).round() as u8)
        })
        .collect();

    // ⚠⚠ `ColorMatrix1` is calibrated at **D65**, not D50. Tagged D50 first, libraw — which
    // reads a colour matrix as D65 whatever the illuminant tag says, as most raw readers do — shifted
    // reds by up to 3.4%. At D65 the matrix is XYZ(D65) → linear ProPhoto through the Bradford-adapted
    // `PROPHOTO_TO_XYZ`, whose white maps to D65 white, so a reader that DOES honour the tag agrees
    // too. `ForwardMatrix1` goes to D50 regardless: the DNG specification defines it that way.
    let xyz_to_pp = inverse(color::spaces::PROPHOTO_TO_XYZ);
    let pp_to_xyz = inverse(color::spaces::XYZ_D50_TO_PROPHOTO);
    let flat = |m: [[f32; 3]; 3]| m.concat();

    // ⚠ Offsets depend on sizes, so the layout is fixed first: header, IFD0, SubIFD, preview,
    // tiles. The two IFDs' sizes do not depend on the offsets they carry.
    let raw_entries = |tile_at: &[u32]| {
        vec![
            longs(254, &[0]),
            longs(256, &[w as u32]),
            longs(257, &[h as u32]),
            shorts(258, &[16, 16, 16]),
            shorts(259, &[7]),
            shorts(262, &[34892]),
            shorts(277, &[3]),
            shorts(284, &[1]),
            longs(322, &[TILE as u32]),
            longs(323, &[TILE as u32]),
            longs(324, tile_at),
            longs(
                325,
                &tiles.iter().map(|t| t.len() as u32).collect::<Vec<_>>(),
            ),
            longs(50714, &[0]),
            longs(50717, &[65535]),
        ]
    };
    let ifd0_entries = |sub_at: u32, preview_at: u32| {
        vec![
            longs(254, &[1]),
            longs(256, &[pw as u32]),
            longs(257, &[ph as u32]),
            shorts(258, &[8, 8, 8]),
            shorts(259, &[1]),
            shorts(262, &[2]),
            ascii(271, "Rustroom"),
            ascii(272, "Rendered"),
            longs(273, &[preview_at]),
            shorts(274, &[1]),
            shorts(277, &[3]),
            longs(278, &[ph as u32]),
            longs(279, &[preview.len() as u32]),
            shorts(284, &[1]),
            ascii(305, "Rustroom"),
            longs(330, &[sub_at]),
            bytes(50706, &[1, 4, 0, 0]),
            bytes(50707, &[1, 1, 0, 0]),
            ascii(50708, "Rustroom Rendered"),
            srationals(50721, &flat(xyz_to_pp)),
            rationals(50728, &[(1, 1), (1, 1), (1, 1)]),
            srationals(50730, &[0.0]),
            shorts(50778, &[21]), // D65 — see `xyz_to_pp`
            ascii(50936, "Rustroom"),
            floats(50940, &[0.0, 0.0, 1.0, 1.0]),
            longs(50941, &[3]), // no restrictions
            srationals(50964, &flat(pp_to_xyz)),
            longs(51110, &[1]), // DefaultBlackRender: none
        ]
    };

    let ifd0_at = 8usize;
    let ifd0_len = ifd_size(&ifd0_entries(0, 0));
    let sub_at = ifd0_at + ifd0_len;
    let sub_len = ifd_size(&raw_entries(&vec![0; tiles.len()]));
    let preview_at = sub_at + sub_len;
    let mut tile_at = Vec::with_capacity(tiles.len());
    let mut at = preview_at + preview.len();
    for t in &tiles {
        tile_at.push(at as u32);
        at += t.len();
    }

    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd0_at as u32).to_le_bytes());
    out.extend(ifd_bytes(
        ifd0_entries(sub_at as u32, preview_at as u32),
        ifd0_at,
    ));
    debug_assert_eq!(out.len(), sub_at);
    out.extend(ifd_bytes(raw_entries(&tile_at), sub_at));
    debug_assert_eq!(out.len(), preview_at);
    out.extend_from_slice(&preview);
    for t in &tiles {
        out.extend_from_slice(t);
    }
    std::fs::write(path, out).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("rustroom_dng_{}_{i}_{name}", std::process::id()))
    }

    /// A colourful, not-tile-aligned picture: gradients across both axes and a saturated patch.
    fn picture(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let c = if (w / 3..w / 2).contains(&x) && (h / 3..h / 2).contains(&y) {
                    color::srgb_to_prophoto([0.8, 0.15, 0.1])
                } else {
                    color::srgb_to_prophoto([u * 0.9, 0.2 + 0.6 * v, (1.0 - u) * 0.5])
                };
                img.set_pixel(x, y, c);
            }
        }
        img
    }

    #[test]
    fn the_inverse_matrix_maps_d50_white_through() {
        let m = inverse(color::spaces::XYZ_D50_TO_PROPHOTO);
        let white = [1.0f32, 1.0, 1.0];
        let xyz: Vec<f32> = (0..3)
            .map(|r| (0..3).map(|c| m[r][c] * white[c]).sum())
            .collect();
        for (got, want) in xyz.iter().zip([0.9642, 1.0, 0.8249]) {
            assert!((got - want).abs() < 1e-3, "{xyz:?}");
        }
    }

    /// ⚠⚠ The independent reader: **libraw**, through Rustroom's own decoder, must open the DNG and
    /// hand back the picture. The bound is measured, not guessed — see the assertion.
    #[test]
    fn libraw_reads_the_dng_back_as_the_picture() {
        let (w, h) = (300, 200); // not a multiple of the tile: edge tiles are exercised
        let mut src = picture(w, h);
        // ⚠⚠ One pure white pixel, and it is load-bearing: libraw's `adjust_maximum` heuristic
        // treats the data's own maximum as white whenever it exceeds 75% of `WhiteLevel`, and
        // without a white pixel it read this picture 1.275× too bright — exactly 1 / 0.784, the
        // brightest sample. A real render contains white; darktable's reader uses the tag.
        src.set_pixel(0, 0, [1.0, 1.0, 1.0]);
        let path = tmp("roundtrip.dng");
        save(&src, &path).unwrap();
        let d = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene());
        let _ = std::fs::remove_file(&path);
        let d = d.expect("libraw must open the DNG");
        assert_eq!((d.image.width(), d.image.height()), (w, h), "size");
        let mut worst = 0f32;
        let mut at = (0, 0);
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (src.pixel(x, y), d.image.pixel(x, y));
                for c in 0..3 {
                    if (a[c] - b[c]).abs() > worst {
                        worst = (a[c] - b[c]).abs();
                        at = (x, y);
                    }
                }
            }
        }
        // ⚠ Measured 5.7e-5 — about 4 of 65,535 levels: 16-bit quantisation and a matrix round
        // trip. It was 0.25 with no white pixel (libraw's `adjust_maximum`) and 0.034 with the
        // colour matrix tagged D50 (libraw reads it as D65); both are recorded where they bite.
        assert!(
            worst < 1e-4,
            "libraw's reading differs by {worst} at {at:?}"
        );
    }
}
