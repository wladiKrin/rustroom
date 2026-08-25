//! libraw-backed decode via the `rsraw` crate.

use crate::{DecodeError, DecodeOptions, DecodedRaw, RawMetadata, Thumbnail};
use core_image::ImageBuf;
use rsraw::{RawImage, ThumbFormat, BIT_DEPTH_16};

const U16_MAX_F: f32 = u16::MAX as f32;

pub(crate) fn extract_thumbnail(bytes: &[u8]) -> Result<Thumbnail, DecodeError> {
    let mut raw =
        RawImage::open(bytes).map_err(|e| DecodeError::Backend(format!("open: {e:?}")))?;
    // The camera orientation libraw would apply during a full decode; embedded thumbnails
    // are stored unrotated, so we have to apply it ourselves to match.
    let flip = raw.as_ref().sizes.flip;
    let thumbs = raw
        .extract_thumbs()
        .map_err(|e| DecodeError::Backend(format!("thumbs: {e:?}")))?;
    // Largest by pixel count (rsraw returns them sorted by height; pick the biggest).
    let t = thumbs
        .into_iter()
        .max_by_key(|t| t.width as u64 * t.height as u64)
        .ok_or_else(|| DecodeError::Format("no embedded thumbnail".into()))?;

    const THUMB_MAX_DIM: u32 = 512;

    let dynimg = match t.format {
        ThumbFormat::Jpeg => image::load_from_memory(&t.data)
            .map_err(|e| DecodeError::Backend(format!("thumb jpeg: {e}")))?,
        ThumbFormat::Bitmap => {
            let (w, h, c) = (t.width as usize, t.height as usize, t.colors as usize);
            if c != 3 || t.data.len() < w * h * c {
                return Err(DecodeError::Format(format!(
                    "unexpected bitmap thumb: {w}x{h} colors={c} len={}",
                    t.data.len()
                )));
            }
            let rgb = image::RgbImage::from_raw(t.width, t.height, t.data[..w * h * c].to_vec())
                .ok_or_else(|| DecodeError::Format("bitmap thumb size mismatch".into()))?;
            image::DynamicImage::ImageRgb8(rgb)
        }
        other => {
            return Err(DecodeError::Format(format!(
                "unsupported thumbnail format: {other:?}"
            )))
        }
    };

    // Downscale to a modest size — the grid draws these at ~160 px, so full-res is wasteful.
    // Rotate after downscaling: same result, a fraction of the pixels to move.
    let scaled = apply_flip(dynimg.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM), flip).to_rgba8();
    Ok(Thumbnail {
        width: scaled.width() as usize,
        height: scaled.height() as usize,
        rgba: scaled.into_raw(),
    })
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let mut raw =
        RawImage::open(bytes).map_err(|e| DecodeError::Backend(format!("open: {e:?}")))?;
    raw.set_half_size(opts.half_size); // must be set before unpack()
    raw.unpack()
        .map_err(|e| DecodeError::Backend(format!("unpack: {e:?}")))?;
    raw.set_use_camera_wb(opts.camera_wb);

    {
        // Poke libraw output params directly (rsraw's high-level API doesn't expose these).
        let params = &mut raw.as_mut().params;
        // ⚠ 4 = ProPhoto (ROMM), the pipeline's working space. libraw applies the camera
        // matrix straight into it, so the widest transform in the chain is one we do not have
        // to write. sRGB (1) clipped saturated colour here, before any slider was touched.
        //
        // ⚠ This number decides what every downstream stage is looking at. Changing it changes
        // every rendered pixel and every golden — it is not a tuning knob.
        // ⚠ 0 = camera-native, 4 = ProPhoto. At 0 libraw still demosaics and still applies the
        // camera white balance; only the colour-space conversion is skipped, which is the step
        // we take over — see `DecodeOptions::camera_native`.
        params.output_color = if opts.camera_native { 0 } else { 4 };
        params.no_auto_bright = i32::from(opts.no_auto_bright);
        // ⚠ Without this libraw uses its own default of 3 (AHD), its slowest — which is what
        // every decode silently inherited before this existed.
        params.user_qual = opts.demosaic.user_qual();
        if opts.linear {
            params.gamm[0] = 1.0; // linear: power = 1.0
            params.gamm[1] = 1.0; // linear: toe slope = 1.0
        }
    }

    // libraw demosaics + applies WB here; gamma/brightness per the params above.
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
    drop(processed);

    // ⚠ Read **after** `dcraw_process`: `rgb_cam` is computed there, so asking earlier returns
    // nothing useful. Probed rather than assumed — see `examples/color_probe.rs`.
    //
    // ⚠ `rgb_cam` is camera → linear **sRGB**, not camera → the requested output space: it comes
    // out identical whatever `output_color` is set to, because dcraw applies the output space
    // separately as `out_cam = out_rgb[output] × rgb_cam`. `camera_to_prophoto` supplies that
    // second half from our own matrices.
    let image = if opts.camera_native {
        let m = {
            let c = &raw.as_mut().color;
            let mut m = [[0.0f32; 3]; 3];
            for (i, row) in m.iter_mut().enumerate() {
                row.copy_from_slice(&c.rgb_cam[i][..3]);
            }
            color::camera::camera_to_prophoto(m)
        };
        ImageBuf::from_vec(width, height, data).map_pixels(|p| color::spaces::apply(&m, p))
    } else {
        ImageBuf::from_vec(width, height, data)
    };

    let metadata = RawMetadata {
        width,
        height,
        make: raw.make().to_string(),
        model: raw.model().to_string(),
    };

    Ok(DecodedRaw { image, metadata })
}

/// Apply libraw's camera orientation code to an embedded thumbnail.
///
/// `decode` gets this for free — libraw rotates during `dcraw_process` — but embedded
/// thumbnails come out of the file unrotated, so a portrait shot would show up sideways in
/// the grid, the filmstrip and the decode placeholder. Using libraw's own `sizes.flip` keeps
/// thumbnail and decode on one source of truth instead of parsing the thumbnail's EXIF.
///
/// Codes follow the dcraw convention: 0 = none, 3 = 180°, 5 = 90° CCW, 6 = 90° CW.
fn apply_flip(img: image::DynamicImage, flip: i32) -> image::DynamicImage {
    match flip {
        3 => img.rotate180(),
        5 => img.rotate270(),
        6 => img.rotate90(),
        _ => img,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    /// 2x1 image: left pixel red, right pixel blue.
    fn wide() -> DynamicImage {
        let mut img = RgbImage::new(2, 1);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        img.put_pixel(1, 0, Rgb([0, 0, 255]));
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn flip_zero_and_unknown_codes_are_identity() {
        for code in [0, 1, 2, 4, 7, -1] {
            let out = apply_flip(wide(), code);
            assert_eq!((out.width(), out.height()), (2, 1), "code {code}");
        }
    }

    #[test]
    fn flip_180_keeps_dimensions_and_swaps_ends() {
        let out = apply_flip(wide(), 3).to_rgb8();
        assert_eq!(out.dimensions(), (2, 1));
        assert_eq!(out.get_pixel(0, 0), &Rgb([0, 0, 255]));
        assert_eq!(out.get_pixel(1, 0), &Rgb([255, 0, 0]));
    }

    #[test]
    fn flip_5_rotates_90_ccw() {
        let out = apply_flip(wide(), 5).to_rgb8();
        assert_eq!(out.dimensions(), (1, 2), "portrait after rotation");
        // Rotating counter-clockwise sends the right-hand (blue) pixel to the top.
        assert_eq!(out.get_pixel(0, 0), &Rgb([0, 0, 255]));
        assert_eq!(out.get_pixel(0, 1), &Rgb([255, 0, 0]));
    }

    #[test]
    fn flip_6_rotates_90_cw() {
        let out = apply_flip(wide(), 6).to_rgb8();
        assert_eq!(out.dimensions(), (1, 2), "portrait after rotation");
        // Clockwise sends the left-hand (red) pixel to the top.
        assert_eq!(out.get_pixel(0, 0), &Rgb([255, 0, 0]));
        assert_eq!(out.get_pixel(0, 1), &Rgb([0, 0, 255]));
    }
}
