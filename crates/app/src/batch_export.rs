//! Exporting a selection of photos to JPEG files.
//!
//! **Sequential on purpose.** The pipeline stages are already rayon-parallel *internally*, so
//! exporting several photos at once would contend for the same cores while multiplying peak
//! memory — a 26 Mpx decode is ~312 MB as f32 RGB, and `PipelineStage::apply` consumes and
//! produces buffers. One at a time keeps the ceiling at roughly one image.
//!
//! The decode fn is **injected**, the same shape `DecodeWorker::new` uses, so the whole loop —
//! naming, collisions, cancellation, progress, the failure path — is testable with no image
//! fixtures at all.

use core_image::ImageBuf;
use edit_model::Edits;
use std::path::{Path, PathBuf};

/// What to export, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub dest_dir: PathBuf,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Fit within this many pixels on the long edge. `None` = full resolution.
    pub max_edge: Option<usize>,
}

/// What to do about one destination that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    Skip,
    Overwrite,
    KeepBoth,
}

/// The user's answer to a collision prompt.
///
/// ⚠ `Cancel` carries **no** `apply_to_all`, on purpose: ticking "apply to all" and then
/// cancelling must arm nothing, and making that unrepresentable is better than enforcing it
/// with a rule someone can forget. Compare `Settings::apply_delete_confirm`, where the flag
/// and the outcome are separate values held together only by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Resolve {
        collision: Collision,
        apply_to_all: bool,
    },
    Cancel,
}

/// Progress from the worker.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportEvent {
    Started {
        total: usize,
    },
    Item {
        done: usize,
        total: usize,
        name: String,
    },
    /// The worker is now **blocked** waiting for a [`Decision`].
    Collision {
        dest: PathBuf,
    },
    Finished(ExportReport),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub exported: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

/// The destination for one source file: `<dir>/<stem>.jpg`, or `<dir>/<stem>-N.jpg`.
///
/// A source with no file stem falls back to `export`, so a pathological path still yields a
/// writable name rather than panicking.
pub fn dest_for(dir: &Path, source: &Path, index: usize) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let name = if index == 0 {
        format!("{stem}.jpg")
    } else {
        format!("{stem}-{index}.jpg")
    };
    dir.join(name)
}

/// The render scale a `max_edge` cap implies for a source whose long edge is `source_long`.
///
/// Mirrors `render_engine::render_scale`, with `source_scale` fixed at 1.0 because an export
/// always decodes at full resolution — never through `DecodeOptions::preview()`.
pub fn scale_for(source_long: usize, max_edge: Option<usize>) -> pipeline::RenderScale {
    let Some(max_edge) = max_edge else {
        return pipeline::RenderScale::FULL;
    };
    if source_long == 0 || max_edge >= source_long {
        // `downscale_to` never upscales, so neither does the scale.
        return pipeline::RenderScale::FULL;
    }
    pipeline::RenderScale::new(max_edge as f32 / source_long as f32)
}

/// Render one decoded image through `edits` and write it as a JPEG.
///
/// ⚠ The downscale happens **before** the pipeline, not after. Rendering at full resolution
/// and shrinking the result would resample **sRGB-encoded** pixels, which is gamma-incorrect;
/// `downscale_to` here operates on the linear decode, where averaging is meaningful. It is
/// also the path the preview already takes, and it is faster, since every stage then runs on
/// fewer pixels.
pub fn export_one(
    image: ImageBuf,
    edits: &Edits,
    dest: &Path,
    opts: &ExportOptions,
) -> Result<(), String> {
    let source_long = image.width().max(image.height());
    let scaled = match opts.max_edge {
        Some(max) => image.downscale_to(max),
        None => image,
    };
    let scale = scale_for(source_long, opts.max_edge);
    let rendered = pipeline::build_pipeline(edits, scale).run(scaled);
    export::save_jpeg(&rendered, dest, opts.quality).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_is_the_stem_plus_jpg() {
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/DSCF6509.RAF"), 0),
            PathBuf::from("/out/DSCF6509.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/holiday.jpeg"), 0),
            PathBuf::from("/out/holiday.jpg")
        );
    }

    #[test]
    fn a_nonzero_index_adds_a_suffix() {
        // What "Keep both" uses: the first free name-1, name-2, ...
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 1),
            PathBuf::from("/out/a-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 7),
            PathBuf::from("/out/a-7.jpg")
        );
    }

    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_export_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A flat mid-gray image of the given size, standing in for a decode.
    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [0.25, 0.25, 0.25]);
            }
        }
        img
    }

    fn opts(dir: &TempDir, max_edge: Option<usize>) -> ExportOptions {
        ExportOptions {
            dest_dir: dir.0.clone(),
            quality: 90,
            max_edge,
        }
    }

    /// Read back a written JPEG's dimensions.
    fn jpeg_size(path: &Path) -> (u32, u32) {
        let img = image::open(path).expect("a readable jpeg");
        (img.width(), img.height())
    }

    #[test]
    fn export_one_writes_a_jpeg_at_full_size_without_a_cap() {
        let d = TempDir::new("one_full");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, None)).unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn the_long_edge_cap_is_honoured() {
        let d = TempDir::new("one_cap");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, Some(10))).unwrap();
        assert_eq!(jpeg_size(&dest), (10, 5));
    }

    #[test]
    fn a_cap_larger_than_the_image_does_not_upscale() {
        // `downscale_to` never upscales; the export must not either.
        let d = TempDir::new("one_big_cap");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, Some(999))).unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn an_unwritable_destination_is_an_error_not_a_panic() {
        let d = TempDir::new("one_bad");
        let dest = d.0.join("no_such_dir").join("a.jpg");
        assert!(export_one(flat(4, 4), &Edits::default(), &dest, &opts(&d, None)).is_err());
    }

    #[test]
    fn the_render_scale_follows_the_downscale() {
        // ⚠ The reason the resize happens *before* the pipeline: the spatial stages have to
        // be told the output resolution, or a resized export would apply full-resolution
        // radii to a small image.
        assert_eq!(scale_for(40, None).get(), 1.0);
        assert_eq!(scale_for(40, Some(10)).get(), 0.25);
        // No upscaling, so a cap above the source is still 1.0.
        assert_eq!(scale_for(40, Some(999)).get(), 1.0);
    }

    #[test]
    fn a_source_with_no_stem_still_produces_a_name() {
        // Defensive: `file_stem` is an Option, and a path ending in `..` has none.
        let dir = PathBuf::from("/out");
        let got = dest_for(&dir, Path::new("/in/.."), 0);
        assert!(got.starts_with("/out"));
        assert_eq!(got.extension().and_then(|e| e.to_str()), Some("jpg"));
    }
}
