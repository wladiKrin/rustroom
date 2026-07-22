//! libraw-backed decode via the `rsraw` crate.

use crate::{DecodeError, DecodedRaw, RawMetadata};
use core_image::ImageBuf;
use rsraw::{RawImage, BIT_DEPTH_16};

const U16_MAX_F: f32 = u16::MAX as f32;

pub(crate) fn decode(bytes: &[u8]) -> Result<DecodedRaw, DecodeError> {
    let mut raw =
        RawImage::open(bytes).map_err(|e| DecodeError::Backend(format!("open: {e:?}")))?;
    raw.unpack()
        .map_err(|e| DecodeError::Backend(format!("unpack: {e:?}")))?;
    // Prefer the camera's own white balance (falls back to auto-WB if unavailable).
    raw.set_use_camera_wb(true);

    // 16-bit output for headroom; libraw demosaics + applies WB + sRGB gamma here.
    let processed = raw
        .process::<{ BIT_DEPTH_16 }>()
        .map_err(|e| DecodeError::Backend(format!("process: {e:?}")))?;

    let width = processed.width() as usize;
    let height = processed.height() as usize;
    let colors = processed.colors() as usize;

    if colors != ImageBuf::CHANNELS {
        return Err(DecodeError::Format(format!(
            "expected {} colors, libraw returned {colors}",
            ImageBuf::CHANNELS
        )));
    }

    // `ProcessedImage<16>` derefs to `&[u16]` of length width*height*colors.
    let samples: &[u16] = &processed;
    let expected = width * height * colors;
    if samples.len() != expected {
        return Err(DecodeError::Format(format!(
            "sample count {} != expected {expected}",
            samples.len()
        )));
    }

    let data: Vec<f32> = samples.iter().map(|&s| s as f32 / U16_MAX_F).collect();
    let image = ImageBuf::from_vec(width, height, data);

    let metadata = RawMetadata {
        width,
        height,
        make: raw.make().to_string(),
        model: raw.model().to_string(),
    };

    Ok(DecodedRaw { image, metadata })
}
