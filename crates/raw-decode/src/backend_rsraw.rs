//! libraw-backed decode via the `rsraw` crate.

use crate::{DecodeError, DecodeOptions, DecodedRaw, RawMetadata, Thumbnail};
use core_image::Capture;
use core_image::ImageBuf;
use rsraw::{RawImage, ThumbFormat, BIT_DEPTH_16};
use std::path::Path;

const U16_MAX_F: f32 = u16::MAX as f32;

/// The largest embedded thumbnail as a decoded image, plus libraw's flip code — not yet
/// oriented, so a caller can shrink first and rotate a fraction of the pixels.
fn largest_embedded(bytes: &[u8]) -> Result<(image::DynamicImage, i32), DecodeError> {
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
    Ok((dynimg, flip))
}

fn to_thumbnail(img: image::RgbaImage) -> Thumbnail {
    Thumbnail {
        width: img.width() as usize,
        height: img.height() as usize,
        rgba: img.into_raw(),
    }
}

pub(crate) fn extract_thumbnail(bytes: &[u8]) -> Result<Thumbnail, DecodeError> {
    let (img, flip) = largest_embedded(bytes)?;
    const THUMB_MAX_DIM: u32 = 512;
    // Downscale to a modest size — the grid draws these at ~160 px, so full-res is wasteful.
    // Rotate after downscaling: same result, a fraction of the pixels to move.
    Ok(to_thumbnail(
        apply_flip(img.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM), flip).to_rgba8(),
    ))
}

/// The largest embedded JPEG at **its own size**, oriented — M9.14's camera preview.
///
/// ⚠ The same extraction as [`extract_thumbnail`] minus the shrink: 4416×2944 on an X-T4,
/// about 4 MB of JPEG, roughly 0.2 s to read and decode.
pub(crate) fn extract_preview(bytes: &[u8]) -> Result<Thumbnail, DecodeError> {
    let (img, flip) = largest_embedded(bytes)?;
    Ok(to_thumbnail(apply_flip(img, flip).to_rgba8()))
}

/// Derive camera → ProPhoto from a `.dcp`, following the DNG spec's dual-illuminant rule.
///
/// ⚠ Returns `None` on any failure — a missing, malformed, or ForwardMatrix-less profile falls
/// back to libraw's matrix rather than failing the decode. A photo that renders slightly
/// differently beats a photo that does not open, and the fallback is the well-tested C1 path.
pub(crate) fn profile_transform(path: &Path, cam_mul: [f32; 3]) -> Option<ProfileTransform> {
    let p = camera_profile::CameraProfile::from_file(path).ok()?;
    let forward1 = p.forward1?;
    let forward2 = p.forward2?;
    let t1 = color::camera::illuminant_temperature(p.illuminant1);
    let t2 = color::camera::illuminant_temperature(p.illuminant2);
    // ⚠ The camera neutral is the **reciprocal** of the white-balance multipliers: `cam_mul`
    // scales the channels up to make a neutral object neutral, so the neutral object's own camera
    // reading is 1/cam_mul. Getting this the right way round is the difference between solving
    // for the scene's temperature and solving for its inverse.
    if cam_mul.iter().any(|v| *v <= 0.0) {
        return None;
    }
    let neutral = [1.0 / cam_mul[0], 1.0 / cam_mul[1], 1.0 / cam_mul[2]];
    let temp = color::camera::neutral_temperature(neutral, p.color1, p.color2, t1, t2);
    let g = color::camera::illuminant_weight(temp, t1, t2);
    // ⚠ **One weight, two consumers.** The matrices and the hue/saturation table are interpolated
    // on the same illuminant weight — using two would let the colour transform and the look
    // disagree about what light the scene was under.
    // ⚠ M10.13a: the forward matrices normalised to D50 as the DNG SDK holds them.
    let matrix = color::camera::forward_to_prophoto(color::camera::blend(
        color::camera::normalize_forward_matrix(forward1),
        color::camera::normalize_forward_matrix(forward2),
        g,
    ));
    // ⚠ Blended **once**, here, outside the pixel loop. The X-T4's table is 2700 entries; doing
    // this per pixel would be 2700 multiplies per pixel for a value that never changes.
    let hue_sat = p.hue_sat_map.as_ref().map(|m| {
        let a = color::huesatmap::Table {
            dims: m.dims,
            data: m.data1.clone(),
        };
        match &m.data2 {
            Some(d2) => color::huesatmap::Table::blend(
                &a,
                &color::huesatmap::Table {
                    dims: m.dims,
                    data: d2.clone(),
                },
                g,
            ),
            None => a,
        }
    });
    Some(ProfileTransform { matrix, hue_sat })
}

/// What a camera profile contributes to the decode: a matrix, and optionally a look-up table.
pub(crate) struct ProfileTransform {
    pub matrix: [[f32; 3]; 3],
    pub hue_sat: Option<color::huesatmap::Table>,
}

/// What libraw knew once it had processed a file, beside the pixels.
pub(crate) struct Facts {
    pub width: usize,
    pub height: usize,
    /// The multipliers `dcraw_process` applied — `cam_mul` scaled so the smallest is 1. ⚠ Read
    /// **after** processing: dcraw divides `pre_mul` in place there (probed on DSCF7618, 2026-10-26).
    pub pre_mul: [f32; 3],
    pub cam_mul: [f32; 3],
    pub cam_xyz: [[f32; 3]; 3],
    pub rgb_cam: [[f32; 3]; 3],
    pub metadata: RawMetadata,
    pub capture: Capture,
    /// The samples are the camera's own colours (libraw converted nothing). ⚠ `false` for a four-colour
    /// sensor even when camera-native was asked — libraw converted it to ProPhoto itself ([`native_output`]).
    pub native: bool,
}

/// Whether libraw hands back camera-native colours: when asked, and only for a three-colour sensor. ⚠ A
/// four-colour one (a CMYG Canon PowerShot Pro70, an RGBE Sony DSC-F828) has no 3×3 camera matrix for us to
/// apply, so libraw converts it into ProPhoto itself — it used to fail with "expected 3 colors" (v0.1).
pub(crate) fn native_output(asked: bool, sensor_colours: i32) -> bool {
    asked && sensor_colours == 3
}

/// Open, unpack and process `bytes` per `opts`, handing libraw's 16-bit camera-space output to
/// `take` while libraw still owns it — so `decode` converts straight to f32 and `decode_camera`
/// copies, and neither pays for the other.
///
/// ⚠ `output_color` 0 or 4 per `opts.camera_native`; `decode_camera` always asks for 0.
pub(crate) fn process<T>(
    bytes: &[u8],
    opts: &DecodeOptions,
    take: impl FnOnce(&[u16]) -> T,
) -> Result<(T, Facts), DecodeError> {
    let mut raw =
        RawImage::open(bytes).map_err(|e| DecodeError::Backend(format!("open: {e:?}")))?;
    raw.set_half_size(opts.half_size); // must be set before unpack()
    raw.unpack()
        .map_err(|e| DecodeError::Backend(format!("unpack: {e:?}")))?;
    raw.set_use_camera_wb(opts.camera_wb);
    let native = native_output(opts.camera_native, raw.as_ref().idata.colors);

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
        params.output_color = if native { 0 } else { 4 };
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
    let taken = take(samples);
    drop(processed);

    // ⚠ Read **after** `dcraw_process`: `rgb_cam` is computed there, so asking earlier returns
    // nothing useful. Probed rather than assumed — see `examples/color_probe.rs`.
    //
    // ⚠ `rgb_cam` is camera → linear **sRGB**, not camera → the requested output space: it comes
    // out identical whatever `output_color` is set to, because dcraw applies the output space
    // separately as `out_cam = out_rgb[output] × rgb_cam`. `camera_to_prophoto` supplies that
    // second half from our own matrices.
    let (pre_mul, cam_mul, cam_xyz, rgb_cam) = {
        let c = &raw.as_mut().color;
        let mut cam_xyz = [[0.0f32; 3]; 3];
        let mut rgb_cam = [[0.0f32; 3]; 3];
        for i in 0..3 {
            cam_xyz[i].copy_from_slice(&c.cam_xyz[i][..3]);
            rgb_cam[i].copy_from_slice(&c.rgb_cam[i][..3]);
        }
        (
            [c.pre_mul[0], c.pre_mul[1], c.pre_mul[2]],
            // ⚠ `cam_mul` is the white balance libraw actually applied, which is what the DNG
            // interpolation is defined against — not a temperature anyone typed.
            [c.cam_mul[0], c.cam_mul[1], c.cam_mul[2]],
            cam_xyz,
            rgb_cam,
        )
    };
    let as_shot = as_shot_illuminant(&cam_xyz, cam_mul);

    let metadata = RawMetadata {
        width,
        height,
        make: raw.make().to_string(),
        model: raw.model().to_string(),
        lens: {
            let l = raw.lens_info();
            let name = l.lens_name.trim().to_string();
            // An empty name is the ordinary case for an adapted or unchipped lens, and must stay
            // `None` rather than becoming an unmatchable empty string.
            if name.is_empty() {
                None
            } else {
                Some(crate::LensMetadata {
                    make: l.lens_make.trim().to_string(),
                    name,
                    focal: raw.focal_len(),
                    aperture: raw.aperture(),
                })
            }
        },
        as_shot,
    };
    let capture = capture_of(&raw);
    Ok((
        taken,
        Facts {
            width,
            height,
            pre_mul,
            cam_mul,
            cam_xyz,
            rgb_cam,
            metadata,
            capture,
            native,
        },
    ))
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let (data, f) = process(bytes, opts, |samples| {
        samples
            .iter()
            .map(|&s| s as f32 / U16_MAX_F)
            .collect::<Vec<f32>>()
    })?;

    let image = if f.native {
        let (m, hue_sat) = match opts
            .camera_profile
            .as_deref()
            .and_then(|p| profile_transform(p, f.cam_mul))
        {
            Some(t) => (t.matrix, t.hue_sat),
            None => (color::camera::camera_to_prophoto(f.rgb_cam), None),
        };
        // ⚠ **One pass, not two.** A second `map_pixels` over a full-resolution buffer would be a
        // second traversal of ~313 MB for nothing.
        ImageBuf::from_vec(f.width, f.height, data).map_pixels(|p| {
            let p = color::spaces::apply(&m, p);
            match &hue_sat {
                Some(t) => t.apply(p),
                None => p,
            }
        })
    } else {
        ImageBuf::from_vec(f.width, f.height, data)
    };

    Ok(DecodedRaw {
        image,
        metadata: f.metadata,
    })
}

/// libraw's camera-space output and what it applied to get there (M10.17a) — see
/// [`crate::decode_camera`].
pub(crate) fn decode_camera(
    bytes: &[u8],
    opts: &DecodeOptions,
) -> Result<crate::CameraFrame, DecodeError> {
    let opts = DecodeOptions {
        camera_native: true,
        camera_profile: None,
        ..opts.clone()
    };
    let (samples, f) = process(bytes, &opts, <[u16]>::to_vec)?;
    if !f.native {
        // A four-colour sensor: libraw converted it, so these are not the camera's colours a merge needs.
        return Err(DecodeError::Format(
            "a four-colour sensor has no camera-space frame".into(),
        ));
    }
    // ⚠ libraw leaves `cam_xyz` empty for a DNG — it builds `rgb_cam` from the file's matrix directly
    // (found by M10.17a's DNG-bracket test: the merge wrote an all-zero ColorMatrix1). Derived back
    // from `rgb_cam`, it gives the same matrix through dcraw's step.
    let cam_xyz = match color::camera::invert(&f.cam_xyz) {
        Some(_) => f.cam_xyz,
        None => color::camera::cam_xyz_from_rgb_cam(&f.rgb_cam).unwrap_or(f.cam_xyz),
    };
    Ok(crate::CameraFrame {
        width: f.width,
        height: f.height,
        samples,
        pre_mul: f.pre_mul,
        cam_xyz,
        rgb_cam: f.rgb_cam,
        metadata: f.metadata,
        capture: f.capture,
    })
}

/// The illuminant the camera balanced to, from the multipliers libraw applied.
///
/// ⚠⚠ **The neutral is the RECIPROCAL of `cam_mul`.** The multipliers scale each channel *up* so
/// that a neutral object comes out neutral, so the object's own camera reading is `1/cam_mul`.
/// Getting this the right way round is the difference between solving for the scene's light and
/// solving for its opposite — the identical note `profile_transform` already carries.
///
/// ⚠⚠ **`cam_xyz` (XYZ → camera), NOT `rgb_cam` — and using the wrong one is not obviously
/// wrong.** `rgb_cam` maps camera → sRGB **with the white balance already applied**, which is
/// exactly why every one of its rows sums to 1: a balanced neutral goes to a balanced neutral. Put
/// `1/cam_mul` through it and the sensor's own green dominance is counted twice. Measured on a
/// real X-T4 frame, that route lands **0.084 off the daylight locus — a tint of 140 on a ±100
/// control** — where `cam_xyz` gives 4762 K at 0.015 off, a plausible illuminant. This is the
/// same matrix and the same reciprocal the DNG spec uses, and that `profile_transform` above
/// already solves against.
pub(crate) fn as_shot_illuminant(
    cam_xyz: &[[f32; 3]; 3],
    cam_mul: [f32; 3],
) -> Option<color::wb::Illuminant> {
    if cam_mul.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let neutral = [1.0 / cam_mul[0], 1.0 / cam_mul[1], 1.0 / cam_mul[2]];
    let xyz = color::spaces::apply(&color::camera::invert(cam_xyz)?, neutral);
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !sum.is_finite() || sum.abs() < 1e-9 {
        return None;
    }
    Some(color::wb::solve_xy((xyz[0] / sum, xyz[1] / sum)))
}

/// Apply libraw's camera orientation code to an embedded thumbnail.
///
/// `decode` gets this for free — libraw rotates during `dcraw_process` — but embedded
/// thumbnails come out of the file unrotated, so a portrait shot would show up sideways in
/// the grid, the filmstrip and the decode placeholder. Using libraw's own `sizes.flip` keeps
/// thumbnail and decode on one source of truth instead of parsing the thumbnail's EXIF.
///
/// Codes follow the dcraw convention: 0 = none, 3 = 180°, 5 = 90° CCW, 6 = 90° CW.
/// The photo's pixel dimensions **as displayed**, without decoding it.
///
/// ⚠⚠ Read **live**, never from the catalog, and that is forced rather than chosen. The metadata
/// scanner queues files with **no capture row**, so adding `width`/`height` columns now would leave
/// every already-scanned file NULL forever — the fields would apply only to photos nobody owns yet.
/// Fixing that needs a `capture_version` column and a re-read pass; reading one photo on demand
/// needs neither.
///
/// ⚠ Costs a whole-file read (~11–15 ms for a RAF) because `RawImage::open` takes a `&[u8]`. Fine
/// for one photo when a dialog opens, and exactly why the *scan* does not do it for every file.
///
/// ⚠ **Swapped for a portrait orientation**, because libraw reports the *sensor's* dimensions.
/// CLAUDE.md records that `sips` doing the same sent an investigation down a blind alley: it
/// called all 411 files landscape while the thumbnails were genuinely portrait.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let raw = RawImage::open(bytes).ok()?;
    let (w, h) = (raw.width(), raw.height());
    let flip = raw.as_ref().sizes.flip;
    // dcraw codes: 5 = 270°, 6 = 90° — the two that transpose.
    Some(if matches!(flip, 5 | 6) {
        (h, w)
    } else {
        (w, h)
    })
}

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

    /// A camera whose native space *is* sRGB — so the fixture's numbers mean something rather
    /// than being an arbitrary matrix — and whose multipliers are equal. Its neutral is then
    /// sRGB's own white, D65. ⚠ It is what makes the fallback for a file with no usable
    /// multipliers a *right answer* rather than a degraded one.
    /// v0.1: only a three-colour sensor stays camera-native; a four-colour one is libraw's to convert.
    #[test]
    fn only_a_three_colour_sensor_stays_camera_native() {
        assert!(native_output(true, 3));
        assert!(!native_output(true, 4));
        assert!(!native_output(false, 3));
    }

    fn srgb_camera() -> [[f32; 3]; 3] {
        color::camera::invert(&color::spaces::SRGB_TO_XYZ).expect("invertible")
    }

    #[test]
    fn equal_multipliers_on_an_srgb_camera_are_d65() {
        let i = as_shot_illuminant(&srgb_camera(), [1.0, 1.0, 1.0]).expect("solves");
        assert!((i.kelvin - 6504.0).abs() < 300.0, "got {}K", i.kelvin);
        assert!(i.tint.abs() < 5.0, "got tint {}", i.tint);
    }

    /// A degenerate matrix must degrade to the D65 fallback rather than producing NaN.
    #[test]
    fn a_singular_camera_matrix_gives_no_illuminant() {
        let flat = [[1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]];
        assert!(as_shot_illuminant(&flat, [1.0, 1.0, 1.0]).is_none());
    }

    /// ⚠⚠ **The reciprocal, pinned by direction rather than by value.** Under warm light the
    /// sensor sees plenty of red, so the camera's red multiplier is *small* — and the neutral it
    /// implies must therefore be a warm, low-Kelvin illuminant. Take `cam_mul` the right way up
    /// instead of its reciprocal and this comes out the other way, which still looks like a
    /// working temperature readout.
    #[test]
    fn a_small_red_multiplier_means_warm_light() {
        let warm = as_shot_illuminant(&srgb_camera(), [1.0, 1.6, 3.0]).expect("solves");
        let cool = as_shot_illuminant(&srgb_camera(), [3.0, 1.6, 1.0]).expect("solves");
        assert!(
            warm.kelvin < 4000.0,
            "a small red multiplier is warm light, got {}K",
            warm.kelvin
        );
        assert!(
            cool.kelvin > 8000.0,
            "a large red multiplier is cool light, got {}K",
            cool.kelvin
        );
    }

    /// A zero or negative multiplier would divide by zero; a file that records none must degrade
    /// to the D65 fallback rather than producing an absurd temperature.
    #[test]
    fn unusable_multipliers_give_no_illuminant() {
        let m = srgb_camera();
        assert!(as_shot_illuminant(&m, [0.0, 1.0, 1.0]).is_none());
        assert!(as_shot_illuminant(&m, [1.0, -1.0, 1.0]).is_none());
        assert!(as_shot_illuminant(&m, [1.0, f32::NAN, 1.0]).is_none());
    }

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

/// Read what the file records about how it was taken, without decoding it.
///
/// ⚠ Measured: ~0.2 ms of parsing on top of reading the file, which is ~11–15 ms for a 29 MB RAF.
/// The read dominates, which is why the caller is a background pass and not the folder scan.
pub fn read_capture(bytes: &[u8]) -> Result<Capture, DecodeError> {
    let raw = RawImage::open(bytes).map_err(|e| DecodeError::Backend(e.to_string()))?;
    Ok(capture_of(&raw))
}

/// The capture facts of an opened file — `read_capture`'s, shared with `process` (M10.17a).
fn capture_of(raw: &RawImage) -> Capture {
    let lens = raw.lens_info();
    Capture {
        // ⚠⚠ `.naive_local()`, NOT the UTC instant. EXIF carries no timezone at all, so storing
        // true instants would make a RAW exact and its JPEG sibling a guess about the machine's
        // zone — the pair could then differ by hours. See `Capture::taken`.
        taken: raw
            .datetime()
            .map(|d| d.naive_local().and_utc().timestamp()),
        camera: Capture::camera_name(&raw.normalized_make(), &raw.normalized_model()),
        lens: non_empty(&lens.lens_name),
        iso: non_zero_u32(raw.iso_speed()),
        shutter: non_zero_f32(raw.shutter()),
        aperture: non_zero_f32(raw.aperture()),
        focal_len: non_zero_f32(raw.focal_len()),
        gps: {
            let g = raw.gps();
            core_image::gps_from_dms(g.latitude, g.longitude)
        },
    }
}

/// ⚠ libraw reports **0** for a value it does not have, not an absence. Storing 0 would put
/// "ISO 0" and "f/0" into a filter as real-looking options.
fn non_zero_u32(v: u32) -> Option<u32> {
    (v != 0).then_some(v)
}

fn non_zero_f32(v: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v)
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn a_missing_value_is_none_rather_than_zero() {
        // ⚠ libraw reports 0 for a value it does not have.
        assert_eq!(non_zero_u32(0), None);
        assert_eq!(non_zero_u32(1600), Some(1600));
        assert_eq!(non_zero_f32(0.0), None);
        assert_eq!(non_zero_f32(f32::NAN), None);
        assert_eq!(non_zero_f32(-1.0), None);
        assert_eq!(non_zero_f32(2.0), Some(2.0));
        assert_eq!(non_empty("  "), None);
        assert_eq!(
            non_empty(" XF23mmF2 R WR "),
            Some("XF23mmF2 R WR".to_string())
        );
    }

    #[test]
    fn a_real_raw_reports_its_dimensions_and_agrees_with_its_jpeg() {
        // ⚠ Gated: it **skips** without the fixture, so a green CI run does not mean it ran.
        //
        // ⚠⚠ **A RAW and its JPEG sibling legitimately differ in SIZE, and that is the finding.**
        // Measured on an X-T4 pair: the RAW is 6246×4170 and the camera's JPEG is 6240×4160 —
        // libraw's active area includes border pixels the camera crops when writing its own JPEG.
        // 6246×4170 is exactly what a full decode produces, so the RAW figure is the right one.
        //
        // ⚠ This is where dimensions differ from camera/date/lens: those are facts about the
        // *shot* and must agree exactly, which is why they are normalized. Dimensions are a
        // property of the *file*, and the two files really are different sizes. The first version
        // of this test asserted equality and was false about correct code.
        //
        // What must hold is the **orientation**: both must agree about which way up it is, or one
        // would report portrait and the other landscape.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            return;
        };
        let raf = std::path::PathBuf::from(&path);
        let (w, h) = crate::dimensions(&raf).expect("no dimensions");
        assert!(w > 0 && h > 0);

        let jpg = raf.with_extension("JPG");
        if jpg.exists() {
            let (jw, jh) = image_decode_dimensions(&jpg).expect("no jpeg dimensions");
            assert_eq!(
                w > h,
                jw > jh,
                "a RAW and its JPEG sibling disagree about the orientation: \
                 {w}x{h} vs {jw}x{jh}"
            );
            // Within 2%: the same photograph, differing only by the sensor border.
            let ratio = (w as f64 / jw as f64 - 1.0).abs();
            assert!(
                ratio < 0.02,
                "the two differ by more than a sensor border: {w}x{h} vs {jw}x{jh}"
            );
        }
    }

    /// ⚠ `raw-decode` must not depend on `image-decode`, so the sibling check shells out to the
    /// same header read `image_decode::dimensions` performs. Kept deliberately small.
    fn image_decode_dimensions(path: &std::path::Path) -> Option<(u32, u32)> {
        image::image_dimensions(path).ok()
    }

    #[test]
    fn a_real_raw_reports_its_capture() {
        // ⚠ Gated on the fixture, so it **skips** without it — a green CI run does not mean this
        // ran. It is the only end-to-end proof either reader has.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            return;
        };
        let c = crate::read_capture(std::path::Path::new(&path)).expect("read");
        assert!(c.taken.is_some(), "no capture time: {c:?}");
        assert!(c.iso.is_some() && c.aperture.is_some(), "{c:?}");
        // ⚠ The normalization, end to end: whatever libraw's casing, the make is uppercase here.
        let cam = c.camera.expect("no camera");
        let make = cam.split_whitespace().next().unwrap();
        assert_eq!(
            make,
            make.to_uppercase(),
            "the make was not normalized: {cam}"
        );
    }
}
