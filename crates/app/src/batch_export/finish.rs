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

/// Sharpen, mark, encode and write `base` — the render at its final size — to `dest`.
pub(super) fn write(
    base: ImageBuf,
    dest: &Path,
    opts: &ExportOptions,
    logo: Option<&export::Logo>,
    meta: &export::FileMetadata,
    plan: &SizePlan,
) -> Result<Written, String> {
    let mut img = base;
    export::output_sharpen::apply(&mut img, &opts.output_sharpening);
    export::watermark::apply(&mut img, &opts.watermark, logo);
    if opts.format == ExportFormat::Dng {
        // ⚠ A DNG is written by `save` and carries no PPI — it is not a print format.
        export::save(&img, dest, opts.format, opts.quality, opts.space)
            .map_err(|e| e.to_string())?;
        export::metadata::embed(dest, opts.format, meta).map_err(|e| e.to_string())?;
    } else {
        let bytes = export::encode(&img, opts.format, opts.quality, opts.space, Some(plan.ppi))
            .and_then(|b| export::metadata::with_xmp(b, opts.format, meta))
            .map_err(|e| e.to_string())?;
        std::fs::write(dest, bytes).map_err(|e| e.to_string())?;
    }
    Ok(Written {
        below_ppi: plan.below_ppi,
        shrunk: false,
    })
}
