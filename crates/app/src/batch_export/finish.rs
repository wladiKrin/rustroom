//! Finishing one exported photo (M10.10b): output sharpening, the watermark, the encode with its
//! PPI and metadata, and the write.
//!
//! ⚠ The order is the feature (`export_one`'s note): sharpening measures the photograph, so it goes
//! before the watermark; the metadata is container surgery, so it goes last.

use super::*;
use core_image::ImageBuf;
use export::sizing::SizePlan;
use export::ExportFormat;
use std::path::Path;

/// What happened to one photo beyond being written — for the report's notes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    /// A print size its pixels could not fill: its file carries a lower PPI than asked.
    pub below_ppi: bool,
    /// Made smaller than asked to fit the file-size limit.
    pub shrunk: bool,
}

/// A file-size limit gives up below this long edge, or after this many shrinks.
const MIN_EDGE: usize = 16;
const MAX_SHRINKS: usize = 8;

/// Sharpen, mark, encode and write `base` — the render at its final size, encoded in `space` — to
/// `dest`.
pub(super) fn write(
    base: ImageBuf,
    dest: &Path,
    opts: &ExportOptions,
    logo: Option<&export::Logo>,
    meta: &export::FileMetadata,
    plan: &SizePlan,
    space: color::OutputSpace,
) -> Result<Written, String> {
    let written = Written {
        below_ppi: plan.below_ppi,
        shrunk: false,
    };
    if opts.format == ExportFormat::Dng {
        // ⚠ A DNG is written by `save` and carries no PPI — it is not a print format.
        let img = finished(base, opts, logo);
        export::save(&img, dest, opts.format, opts.quality, opts.space)
            .map_err(|e| e.to_string())?;
        export::metadata::embed(dest, opts.format, meta).map_err(|e| e.to_string())?;
        return Ok(written);
    }
    if !(opts.limit.on && opts.format.is_lossy()) {
        let img = finished(base, opts, logo);
        let bytes = export::encode(&img, opts.format, opts.quality, opts.space, Some(plan.ppi))
            .and_then(|b| export::metadata::with_xmp(b, opts.format, meta))
            .map_err(|e| e.to_string())?;
        std::fs::write(dest, bytes).map_err(|e| e.to_string())?;
        return Ok(written);
    }
    within_limit(base, dest, opts, logo, meta, plan, space)
        .map(|shrunk| Written { shrunk, ..written })
}

/// Output sharpening, then the watermark — on the picture at the size it is written at.
fn finished(mut img: ImageBuf, opts: &ExportOptions, logo: Option<&export::Logo>) -> ImageBuf {
    export::output_sharpen::apply(&mut img, &opts.output_sharpening);
    export::watermark::apply(&mut img, &opts.watermark, logo);
    img
}

/// Write the JPEG at the highest quality within the limit, down to the floor; past it, smaller.
/// Returns whether the picture was made smaller.
///
/// ⚠⚠ **Shrunk from `base`, the render BEFORE sharpening and the watermark, and both redone** —
/// sharpening exists to restore what a resize took, and a watermark is sized from the frame, so
/// neither may be resized with the photograph. And always from `base` itself, never from the last
/// attempt, so the rounds do not resample a resample.
fn within_limit(
    base: ImageBuf,
    dest: &Path,
    opts: &ExportOptions,
    logo: Option<&export::Logo>,
    meta: &export::FileMetadata,
    plan: &SizePlan,
    space: color::OutputSpace,
) -> Result<bool, String> {
    let limit = opts.limit.bytes();
    let (bw, bh) = (base.width(), base.height());
    let mut scale = 1.0f64;
    for round in 0..=MAX_SHRINKS {
        let sized = if round == 0 {
            base.clone()
        } else {
            let w = ((bw as f64 * scale).round() as usize).max(1);
            let h = ((bh as f64 * scale).round() as usize).max(1);
            if w.max(h) < MIN_EDGE {
                break;
            }
            export::sizing::resample_encoded(&base, space, w, h)
        };
        let img = finished(sized, opts, logo);
        let found = export::size_limit::encode_within(limit, |q| {
            export::encode(&img, opts.format, q, opts.space, Some(plan.ppi))
                .and_then(|b| export::metadata::with_xmp(b, opts.format, meta))
        })
        .map_err(|e| e.to_string())?;
        match found {
            export::size_limit::Within::Fits { bytes, .. } => {
                std::fs::write(dest, bytes).map_err(|e| e.to_string())?;
                return Ok(round > 0);
            }
            // ⚠ A file's size goes with its pixel count, so the side shrinks by the square root of
            // the overshoot — with a margin, since the metadata does not shrink with it.
            export::size_limit::Within::TooBig { at_floor } => {
                scale *= ((limit as f64 / at_floor as f64).sqrt() * 0.95).min(0.95);
            }
        }
    }
    Err(format!(
        "could not be made smaller than {} KB",
        opts.limit.kb
    ))
}
