//! A file-size limit (M10.10b): the JPEG quality searched until the file fits.
//!
//! ⚠ JPEG only — the one lossy format, so the only one with a quality to give. ⚠⚠ **What is measured
//! is the whole file**, the XMP packet included: the caller's `encode` closure returns the bytes as
//! they will be written, so a caption cannot push a "fitting" file over its limit.

use crate::ExportError;
use serde::{Deserialize, Serialize};

/// The lowest quality the search goes to; below it a picture is made smaller instead — the author's
/// choice, 2026-10-11, over a JPEG too blocky to use.
pub const QUALITY_FLOOR: u8 = 40;

/// Limit file size to `kb` — when `on`.
///
/// ⚠ **KB is 1000 bytes**, the stricter reading: a file within it meets the limit whichever a
/// website meant. ⚠ The number is kept while the limit is off, so ticking it again restores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSizeLimit {
    pub on: bool,
    pub kb: u32,
}

impl Default for FileSizeLimit {
    fn default() -> Self {
        Self { on: false, kb: 500 }
    }
}

impl FileSizeLimit {
    pub const KB_RANGE: (u32, u32) = (1, 100_000);

    pub fn bytes(self) -> usize {
        self.kb as usize * 1000
    }
}

/// What the search found.
#[derive(Debug)]
pub enum Within {
    /// The highest quality that fits, and its bytes.
    Fits { bytes: Vec<u8>, quality: u8 },
    /// Even [`QUALITY_FLOOR`] is too big: it came to `at_floor` bytes.
    TooBig { at_floor: usize },
}

/// The highest quality in `QUALITY_FLOOR..=100` whose file is at most `limit` bytes — a binary
/// search, seven encodes at most.
///
/// ⚠ JPEG size is not strictly monotone in the quality, so this is the highest a bisection reaches,
/// with the quality one above it known to be over — never a file over the limit.
pub fn encode_within(
    limit: usize,
    encode: impl Fn(u8) -> Result<Vec<u8>, ExportError>,
) -> Result<Within, ExportError> {
    let floor = encode(QUALITY_FLOOR)?;
    if floor.len() > limit {
        return Ok(Within::TooBig {
            at_floor: floor.len(),
        });
    }
    let mut best = (QUALITY_FLOOR, floor);
    let (mut lo, mut hi) = (QUALITY_FLOOR + 1, 100u8);
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let bytes = encode(mid)?;
        if bytes.len() <= limit {
            best = (mid, bytes);
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    Ok(Within::Fits {
        bytes: best.1,
        quality: best.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode, ExportFormat};
    use color::OutputSpace;
    use core_image::ImageBuf;

    #[test]
    fn the_highest_quality_that_fits_is_kept() {
        let by_quality = |q: u8| Ok(vec![0u8; q as usize * 1000]);
        match encode_within(60_000, by_quality).unwrap() {
            Within::Fits { bytes, quality } => assert_eq!((quality, bytes.len()), (60, 60_000)),
            other => panic!("{other:?}"),
        }
        match encode_within(1_000_000, by_quality).unwrap() {
            Within::Fits { quality, .. } => assert_eq!(quality, 100),
            other => panic!("{other:?}"),
        }
        match encode_within(39_999, by_quality).unwrap() {
            Within::TooBig { at_floor } => assert_eq!(at_floor, 40_000),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_real_jpeg_lands_under_its_limit_with_the_next_quality_over_it() {
        // ⚠ Noise, so the size depends on the quality the way a photograph's does.
        let mut img = ImageBuf::new(300, 200);
        let mut s = 12345u32;
        for y in 0..200 {
            for x in 0..300 {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let v = (s >> 8) as f32 / (1u32 << 24) as f32;
                img.set_pixel(x, y, [v, 0.5 * v + 0.25, 1.0 - v]);
            }
        }
        let jpeg = |q: u8| encode(&img, ExportFormat::Jpeg, q, OutputSpace::Srgb, None);
        let limit = jpeg(70).unwrap().len();
        let Within::Fits { bytes, quality } = encode_within(limit, jpeg).unwrap() else {
            panic!("quality 70 fits by construction");
        };
        assert!(bytes.len() <= limit);
        assert!(quality >= 70, "{quality}");
        assert!(
            quality == 100 || jpeg(quality + 1).unwrap().len() > limit,
            "{quality} is not the highest that fits"
        );
    }

    #[test]
    fn a_limit_is_in_thousands_of_bytes_and_off_by_default() {
        let d = FileSizeLimit::default();
        assert!(!d.on);
        assert_eq!(d.kb, 500);
        assert_eq!(FileSizeLimit { on: true, kb: 2 }.bytes(), 2000);
    }
}
