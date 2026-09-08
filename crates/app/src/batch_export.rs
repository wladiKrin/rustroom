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

use crate::filename_template::{NameFacts, Template};
use crate::photo_id::PhotoId;
use core_image::ImageBuf;
use edit_model::Edits;
use export::ExportFormat;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// One photo to export: which version of which file, what it is called, and its edits.
///
/// ⚠ The name is carried *here* rather than looked up in the sidecar by the worker, because the
/// worker runs on its own thread and must not race the UI writing that file — the same reason the
/// edits travel with the job instead of being re-read.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub id: PhotoId,
    /// This copy's name, when it has one. `None` on an original, which has no such name.
    pub name: Option<String>,
    pub edits: Edits,
    /// What the catalog knows about how the shot was taken, for `{date}` and `{camera}`.
    ///
    /// ⚠ Carried like the name and the edits, and for the same reason: the worker runs on its
    /// own thread and has no catalog connection. ⚠ An all-`None` `Capture` is the ordinary answer
    /// for a file with no EXIF and for single-file mode, where there is no Library to ask.
    pub capture: core_image::Capture,
}

/// What to export, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    pub dest_dir: PathBuf,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Fit within this many pixels on the long edge. `None` = full resolution.
    pub max_edge: Option<usize>,
    /// Demosaic quality for RAW sources.
    ///
    /// ⚠ An **export** setting rather than a develop edit, and by structure rather than by
    /// preference: the editing preview decodes `half_size`, which skips demosaic entirely, so
    /// this can only ever affect a full-resolution decode.
    pub demosaic: raw_decode::Demosaic,
    /// The output format and bit depth.
    pub format: ExportFormat,
    /// The colour space the file is written in.
    ///
    /// ⚠ Export-only. The preview stays sRGB, because the display is assumed sRGB until a
    /// display profile exists — see `pipeline::OutputTransform`.
    ///
    /// ⚠ **No field-level `#[serde(default)]` here, and that is not an omission.** The struct
    /// carries a *container-level* one, so a missing field already falls back to this struct's
    /// `Default` — adding it per field implies a guarantee it is not the source of.
    pub space: color::OutputSpace,

    // ─── M6.1: what turns an export into a deliverable ────────────────────────────────────
    //
    // ⚠⚠ The five fields below are **one seam, landed together and deliberately**, so that the
    // sub-features can be built independently without each having to edit this struct — which
    // is the shared surface every one of them would otherwise contend for. Four of them are
    // *inert* today: they are stored, defaulted, round-tripped and read by nothing.
    //
    // ⚠ Every one defaults to "do nothing", so an existing user's next export is unchanged.
    // The container-level `#[serde(default)]` means an `ExportOptions` written before any of
    // them still loads — `an_options_file_from_before_the_delivery_fields_still_loads`.
    /// The base filename, as a template — see [`crate::filename_template`]. **M6.1a, live.**
    ///
    /// ⚠ The default renders to exactly what `dest_for` produced before templating existed, so
    /// nothing about an existing export changes until someone edits this.
    pub filename_template: String,
    /// IPTC/XMP fields to write into the exported file. **M6.1b, live.**
    ///
    /// ⚠ Written **into the export only** — the original is never opened for writing. See
    /// `export::metadata`, which carries why that dissolved the backlog's design conflict.
    pub metadata: ExportMetadata,
    /// A visible watermark laid over the exported image. **M6.1c, live.**
    pub watermark: Watermark,
    /// Sharpening applied for the output medium, after any resize. **M6.1d, live.**
    pub output_sharpening: OutputSharpening,
    /// Where the files land and what happens afterwards. **M6.1e — stored, inert.**
    pub delivery: Delivery,
}

// ⚠⚠ These three **live in the `export` crate** (M6.1b/c/d) and are re-exported here, so no call
// site churned. They belong beside `save`: one is byte surgery on the container `save` has just
// written, and the other two act on the buffer `save` is about to quantize. None of them has
// anything to do with the batch loop, and putting them there would have left the CLI unable to
// reach any of it. Re-exported rather than written `export::…` at every use site, because they
// are `ExportOptions` fields first.
pub use export::{ExportMetadata, OutputSharpening, Watermark, WatermarkCorner};

/// Where the files land inside the destination, and what happens once the run finishes.
///
/// ⚠ An empty `subfolder` means "straight into the destination". **M6.1e owns this type.**
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delivery {
    /// Created under `dest_dir` if non-empty. ⚠ Must go through `safe_name`, like every other
    /// typed string that reaches the filesystem.
    pub subfolder: String,
    pub then: PostExportAction,
}

/// What to do when a run finishes. ⚠ **M6.1e owns this** — append variants, never insert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostExportAction {
    /// The behaviour every export has had until now.
    #[default]
    Nothing,
    RevealInFileManager,
}

/// The template every export used before templating existed, and the default.
///
/// ⚠ It must render to exactly what `dest_for` produces, so an existing user's next export is
/// byte-identical — see `the_default_template_reproduces_the_old_naming`.
pub const DEFAULT_FILENAME_TEMPLATE: &str = "{name}_{copy}";

/// The banding warning for a space/format pair, or `None` when there is nothing to say.
///
/// ⚠ **Warn, never block.** A blocked combination reads as "the thing I didn't click went dark",
/// which this project has already shipped once and taken back out. The state this warns about is
/// one click from being fixed.
pub fn banding_warning(space: color::OutputSpace, format: ExportFormat) -> Option<&'static str> {
    let eight_bit = matches!(
        format,
        ExportFormat::Jpeg | ExportFormat::Png8 | ExportFormat::Tiff8
    );
    (space.warns_in_8_bit() && eight_bit)
        .then_some("ProPhoto in 8-bit will band on smooth gradients — use 16-bit TIFF or PNG.")
}

impl Default for ExportOptions {
    /// ⚠ Needed for `#[serde(default)]` on `Settings` to fill this field when loading a
    /// config written before it existed. Without it every existing user's settings file
    /// would fail to parse — and `SettingsStore::load()` being infallible would turn that
    /// into *all* settings silently reverting, which is worse than an error.
    ///
    /// An empty destination means "never chosen", which `destination_ready` reports as not
    /// ready, so a fresh install still has to pick a folder.
    fn default() -> Self {
        Self {
            dest_dir: PathBuf::new(),
            quality: 92,
            max_edge: None,
            demosaic: raw_decode::Demosaic::default(),
            format: ExportFormat::default(),
            space: color::OutputSpace::default(),
            // ⚠ Not `String::default()`. The neutral filename template is `{name}`, which is
            // what reproduces the naming every export had before templating; an empty template
            // would fall back to the stem too, but only by accident rather than by rule.
            filename_template: DEFAULT_FILENAME_TEMPLATE.to_string(),
            metadata: ExportMetadata::default(),
            watermark: Watermark::default(),
            output_sharpening: OutputSharpening::default(),
            delivery: Delivery::default(),
        }
    }
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
/// Where one export lands.
///
/// ⚠ **Two numbers, two meanings, two separators.** `_N` is which virtual copy this is — a
/// property of the photo, stable forever, and only the *default* for an unnamed copy. `-N` is how
/// many files already wanted this name in this folder — a property of the destination, assigned at
/// export time. Merging them into one scheme would make `a-2.jpg` ambiguous between "copy 2" and
/// "the third file called a", and would silently shift a copy's number whenever its plain name
/// happened to be taken.
///
/// ⚠ Two copies can be given the *same* name; nothing prevents it, and `-N` is what stops them
/// overwriting each other. That is the second reason the schemes stay separate.
pub fn dest_for(
    dir: &Path,
    source: &Path,
    copy: u32,
    copy_name: Option<&str>,
    dedup: usize,
    format: ExportFormat,
) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    // ⚠ The default template, so this legacy entry point and the templated path cannot disagree
    // about what a copy is called. `export_stem` is gone.
    let base = Template::default().render(&NameFacts {
        file_stem: stem,
        copy,
        copy_label: copy_name,
        ..NameFacts::default()
    });
    dest_named(dir, &base, dedup, format)
}

/// Where an export lands, given the base name already chosen for it.
///
/// ⚠ This is the half that owns the **de-duplication** suffix and the extension, and nothing
/// else. Splitting it out is what lets a filename template decide the base without learning
/// about `-N` — which must stay separate from both `_N` (copy identity) and the template, for
/// the reasons [`dest_for`] gives.
pub fn dest_named(dir: &Path, base: &str, dedup: usize, format: ExportFormat) -> PathBuf {
    let ext = format.extension();
    let name = if dedup == 0 {
        format!("{base}.{ext}")
    } else {
        format!("{base}-{dedup}.{ext}")
    };
    dir.join(name)
}

/// Whether a destination directory can be exported into.
///
/// Empty means never chosen; anything else means chosen once and possibly gone since — an
/// unplugged drive, a deleted folder, or a preset carried from another machine.
///
/// Checked **before** a run rather than per photo. `run` already survives an unwritable
/// destination without panicking, but failing two hundred photos one at a time and reporting
/// afterwards is a poor trade when the answer is knowable up front.
pub fn destination_ready(dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && dir.is_dir()
}

/// Whether a run can start at all: a usable destination, and a template that parses.
///
/// ⚠ A pure function rather than a condition inside the egui closure, for `destination_ready`'s
/// own reason — that is what makes "Export goes dark when the template is wrong" something a test
/// can see rather than something to verify by clicking. ⚠ And it is the place any *later*
/// pre-flight check belongs (a subfolder that will not sanitize, say), so the dialog keeps asking
/// one question.
pub fn can_export(opts: &ExportOptions) -> bool {
    destination_ready(&opts.dest_dir) && Template::parse(&opts.filename_template).is_ok()
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
    resampler: pipeline::Resampler,
    wb_baseline: color::wb::Illuminant,
) -> Result<(), String> {
    let source_long = image.width().max(image.height());
    let scaled = match opts.max_edge {
        Some(max) => image.downscale_to(max),
        None => image,
    };
    let scale = scale_for(source_long, opts.max_edge);
    let mut rendered = pipeline::build_pipeline_spec(
        edits,
        scale,
        pipeline::RenderSpec {
            space: opts.space,
            resampler,
            baseline: wb_baseline,
        },
    )
    .run(scaled);

    // ⚠⚠ **THE ORDER OF THESE THREE IS THE FEATURE, NOT AN ARRANGEMENT.**
    //
    // Output sharpening goes **first**, because it exists to put back the edge the resize took
    // and must therefore measure the photograph — a watermark laid down first would be a hard
    // white edge for it to ring around. Both act on the display-encoded buffer, which is what
    // the pipeline has just handed back; see each module for why that is right for it.
    //
    // The metadata goes **last**, after the encode, because it is surgery on the container
    // rather than on the pixels. It cannot change what the photograph looks like.
    export::output_sharpen::apply(&mut rendered, &opts.output_sharpening);
    export::watermark::apply(&mut rendered, &opts.watermark);
    export::save(&rendered, dest, opts.format, opts.quality, opts.space)
        .map_err(|e| e.to_string())?;
    export::metadata::embed(dest, opts.format, &opts.metadata).map_err(|e| e.to_string())
}

/// A running batch export.
///
/// Owns a thread that works through the jobs one at a time. The thread **blocks** on
/// `decision_rx` whenever it needs a collision answered, which is why the UI must drain
/// `events()` every frame and reply with [`Self::decide`].
pub struct ExportWorker {
    events: Receiver<ExportEvent>,
    decisions: Sender<Decision>,
    cancel: Arc<AtomicBool>,
}

impl ExportWorker {
    /// Start exporting `jobs` into `opts.dest_dir`.
    ///
    /// `cancel` is an input rather than something built here so that a test can flip it from
    /// *inside* the injected decode fn, which is the only way to exercise the mid-photo
    /// cancel check deterministically.
    ///
    /// ⚠ `resampler` is a parameter rather than a field of [`ExportOptions`], and that is the
    /// whole reason there is one setting instead of two: in `ExportOptions` it would be saved
    /// into every export preset, so a preset could disagree with the preview about which kernel
    /// the photograph is rendered with. Two facts that can disagree.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        jobs: Vec<ExportJob>,
        opts: ExportOptions,
        decode: impl Fn(&Path) -> Result<(ImageBuf, color::wb::Illuminant), String> + Send + 'static,
        cancel: Arc<AtomicBool>,
        resampler: pipeline::Resampler,
    ) -> Self {
        let (event_tx, events) = channel();
        let (decisions, decision_rx) = channel();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            run(jobs, opts, decode, event_tx, decision_rx, flag, resampler);
        });
        Self {
            events,
            decisions,
            cancel,
        }
    }

    pub fn events(&self) -> &Receiver<ExportEvent> {
        &self.events
    }

    /// Answer the outstanding [`ExportEvent::Collision`], unblocking the worker.
    pub fn decide(&self, decision: Decision) {
        let _ = self.decisions.send(decision);
    }

    /// Ask the worker to stop.
    ///
    /// **The photo in flight is not exported.** libraw's `unpack` cannot be interrupted, so
    /// its decode runs to completion on the worker thread — but the result is discarded
    /// rather than written, and the caller can drop this `ExportWorker` immediately instead
    /// of waiting. That is the same "abandon, don't abort" trade `DecodeWorker` makes for
    /// superseded previews: identical outcome to a true abort, no unsafe FFI.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    jobs: Vec<ExportJob>,
    opts: ExportOptions,
    decode: impl Fn(&Path) -> Result<(ImageBuf, color::wb::Illuminant), String>,
    events: Sender<ExportEvent>,
    decisions: Receiver<Decision>,
    cancel: Arc<AtomicBool>,
    resampler: pipeline::Resampler,
) {
    let total = jobs.len();
    let mut report = ExportReport::default();
    // Names produced during this run. ⚠ Without this, two sources sharing a stem
    // (`a.RAF`, `a.jpg`) would silently overwrite each other in an empty folder.
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut remembered: Option<Collision> = None;
    // ⚠ Parsed **once** per run, not per photo. ⚠ A template that will not parse falls back to
    // the default rather than failing the run: the dialog refuses to start an export with an
    // invalid one, so the only way to get here is a hand-edited preset file — and a folder named
    // the old way beats no folder at all.
    let template = Template::parse(&opts.filename_template).unwrap_or_default();

    if events.send(ExportEvent::Started { total }).is_err() {
        return;
    }

    for (
        done,
        ExportJob {
            id,
            name,
            edits,
            capture,
        },
    ) in jobs.into_iter().enumerate()
    {
        let source = id.path.clone();
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        // ⚠ `{seq}` is 1-based and positional in the job list, which is the order the Library is
        // showing. It is assigned here, *before* the collision check, so a skipped photo does not
        // renumber the ones after it — a name is a property of the photo, not of what happened to
        // its neighbours.
        let base = template.render(&NameFacts {
            file_stem: source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("export"),
            copy: id.copy,
            copy_label: name.as_deref(),
            seq: done + 1,
            taken: capture.taken,
            camera: capture.camera.as_deref(),
        });
        let mut dest = dest_named(&opts.dest_dir, &base, 0, opts.format);
        if claimed.contains(&dest) || dest.exists() {
            let collision = match remembered {
                Some(c) => c,
                None => {
                    if events
                        .send(ExportEvent::Collision { dest: dest.clone() })
                        .is_err()
                    {
                        return;
                    }
                    // Blocks until the UI answers. A disconnected channel means the app is
                    // going away, so stop rather than spin.
                    match decisions.recv() {
                        Ok(Decision::Resolve {
                            collision,
                            apply_to_all,
                        }) => {
                            if apply_to_all {
                                remembered = Some(collision);
                            }
                            collision
                        }
                        Ok(Decision::Cancel) | Err(_) => {
                            report.cancelled = true;
                            break;
                        }
                    }
                }
            };
            match collision {
                Collision::Skip => {
                    report.skipped += 1;
                    let _ = events.send(ExportEvent::Item {
                        done: done + 1,
                        total,
                        name: file_label(&source),
                    });
                    continue;
                }
                Collision::Overwrite => {}
                Collision::KeepBoth => {
                    let mut i = 1;
                    loop {
                        // ⚠ The *same* base, so "Keep both" appends `-N` to whatever the
                        // template produced rather than quietly falling back to the old naming.
                        let candidate = dest_named(&opts.dest_dir, &base, i, opts.format);
                        if !claimed.contains(&candidate) && !candidate.exists() {
                            dest = candidate;
                            break;
                        }
                        i += 1;
                    }
                }
            }
        }

        let _ = events.send(ExportEvent::Item {
            done: done + 1,
            total,
            name: file_label(&source),
        });

        let decoded = decode(&source);

        // ⚠ Checked *again* after the decode, which is the slow part. Cancelling during a
        // ~11 s decode must not still write that photo out: libraw cannot be interrupted, so
        // the decode finishes, but its result is discarded here. Without this the file the
        // user cancelled would appear anyway.
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        match decoded
            .and_then(|(img, baseline)| export_one(img, &edits, &dest, &opts, resampler, baseline))
        {
            Ok(()) => {
                report.exported += 1;
                claimed.insert(dest);
            }
            Err(e) => report.failed.push((source, e)),
        }
    }

    let _ = events.send(ExportEvent::Finished(report));
}

/// A short display name for the progress line.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_is_the_stem_plus_jpg() {
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6509.RAF"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            PathBuf::from("/out/DSCF6509.jpg")
        );
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/holiday.jpeg"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            PathBuf::from("/out/holiday.jpg")
        );
    }

    #[test]
    fn a_nonzero_index_adds_a_suffix() {
        // What "Keep both" uses: the first free name-1, name-2, ...
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 1, ExportFormat::Jpeg),
            PathBuf::from("/out/a-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 7, ExportFormat::Jpeg),
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
            ..ExportOptions::default()
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
        export_one(
            flat(40, 20),
            &Edits::default(),
            &dest,
            &opts(&d, None),
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn the_long_edge_cap_is_honoured() {
        let d = TempDir::new("one_cap");
        let dest = d.0.join("a.jpg");
        export_one(
            flat(40, 20),
            &Edits::default(),
            &dest,
            &opts(&d, Some(10)),
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (10, 5));
    }

    #[test]
    fn a_cap_larger_than_the_image_does_not_upscale() {
        // `downscale_to` never upscales; the export must not either.
        let d = TempDir::new("one_big_cap");
        let dest = d.0.join("a.jpg");
        export_one(
            flat(40, 20),
            &Edits::default(),
            &dest,
            &opts(&d, Some(999)),
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        )
        .unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    /// A hard vertical edge on a mid-grey field, big enough to hold a watermark.
    fn edged(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, if x < w / 2 { [0.20; 3] } else { [0.55; 3] });
            }
        }
        img
    }

    /// Export `edged` twice — once neutral, once with `tweak` applied to the options — and
    /// report the written bytes of each.
    ///
    /// ⚠⚠ **THESE THREE TESTS EXIST BECAUSE THREE SABOTAGES WENT UNCAUGHT.** Deleting each of
    /// `export_one`'s three new calls left the entire suite green: `export::metadata`,
    /// `export::watermark` and `export::output_sharpen` are all thoroughly tested *in their own
    /// crate*, and nothing anywhere asserted that the export path actually reaches them. That is
    /// the "feature silently absent" failure this project has already shipped once, in
    /// edit-accurate thumbnails.
    fn exported_with(tag: &str, tweak: impl FnOnce(&mut ExportOptions)) -> (Vec<u8>, Vec<u8>) {
        // ⚠ TIFF, not JPEG: a lossy encoder quantizes a faint watermark or a one-pixel sharpen
        // halo into the same blocks, so "the bytes differ" would be measuring the encoder.
        let d = TempDir::new(tag);
        let mut base = opts(&d, None);
        base.format = ExportFormat::Tiff8;
        let plain = d.0.join("plain.tif");
        export_one(
            edged(160, 120),
            &Edits::default(),
            &plain,
            &base,
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        )
        .unwrap();
        let mut changed = base.clone();
        tweak(&mut changed);
        let other = d.0.join("other.tif");
        export_one(
            edged(160, 120),
            &Edits::default(),
            &other,
            &changed,
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        )
        .unwrap();
        (
            std::fs::read(&plain).unwrap(),
            std::fs::read(&other).unwrap(),
        )
    }

    /// The decoded pixels of a written file.
    fn pixels(bytes: &[u8]) -> Vec<u8> {
        image::load_from_memory(bytes).unwrap().to_rgb8().into_raw()
    }

    #[test]
    fn an_export_carries_its_metadata() {
        let (plain, tagged) = exported_with("e2e_meta", |o| {
            o.metadata.copyright = "(c) 2026 Rustroom".into()
        });
        let needle = b"(c) 2026 Rustroom";
        assert!(
            !plain.windows(needle.len()).any(|w| w == needle),
            "fixture check: the neutral export already carried it"
        );
        assert!(
            tagged.windows(needle.len()).any(|w| w == needle),
            "the copyright never reached the file"
        );
        // ⚠ And only the container moved: metadata is written after the encode, so it cannot
        // change what the photograph looks like.
        assert_eq!(
            pixels(&plain),
            pixels(&tagged),
            "the metadata moved a pixel"
        );
    }

    #[test]
    fn an_export_carries_its_watermark() {
        let (plain, marked) = exported_with("e2e_mark", |o| o.watermark.text = "Rustroom".into());
        assert_ne!(
            pixels(&plain),
            pixels(&marked),
            "the watermark never reached the pixels"
        );
    }

    #[test]
    fn an_export_carries_its_output_sharpening() {
        let (plain, sharp) = exported_with("e2e_sharp", |o| o.output_sharpening.amount = 100);
        let (a, b) = (pixels(&plain), pixels(&sharp));
        assert_ne!(a, b, "output sharpening never reached the pixels");
        // ⚠ And it is *sharpening*, not a global lift: the flat halves must be untouched while
        // the edge moves. Without this the test would pass for any change at all — including a
        // watermark, which is the other thing this call site could have been wired to.
        let at = |v: &[u8], x: usize| v[(60 * 160 + x) * 3] as i32;
        assert_eq!(at(&a, 5), at(&b, 5), "the flat left half moved");
        assert_eq!(at(&a, 155), at(&b, 155), "the flat right half moved");
        assert!(
            at(&b, 80) > at(&a, 80) && at(&b, 79) < at(&a, 79),
            "no overshoot/undershoot at the edge: {:?} vs {:?}",
            (at(&a, 79), at(&a, 80)),
            (at(&b, 79), at(&b, 80))
        );
    }

    #[test]
    fn an_unwritable_destination_is_an_error_not_a_panic() {
        let d = TempDir::new("one_bad");
        let dest = d.0.join("no_such_dir").join("a.jpg");
        assert!(export_one(
            flat(4, 4),
            &Edits::default(),
            &dest,
            &opts(&d, None),
            pipeline::Resampler::default(),
            color::wb::Illuminant::default()
        )
        .is_err());
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

    /// Drive a whole export to completion, answering every collision prompt with `answer`.
    ///
    /// Returns the report plus every event, so a test can assert on progress as well as the
    /// outcome. `decode` receives the source path so a test can fail one specific photo.
    fn run_export(
        jobs: Vec<ExportJob>,
        opts: ExportOptions,
        answer: Vec<Decision>,
        decode: impl Fn(&Path) -> Result<(ImageBuf, color::wb::Illuminant), String> + Send + 'static,
    ) -> (ExportReport, Vec<ExportEvent>) {
        let worker = ExportWorker::spawn(
            jobs,
            opts,
            decode,
            Arc::new(AtomicBool::new(false)),
            pipeline::Resampler::default(),
        );
        let mut answers = answer.into_iter();
        let mut events = Vec::new();
        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            match &ev {
                ExportEvent::Collision { .. } => {
                    let d = answers.next().unwrap_or(Decision::Cancel);
                    worker.decide(d);
                }
                ExportEvent::Finished(r) => report = r.clone(),
                _ => {}
            }
            let finished = matches!(ev, ExportEvent::Finished(_));
            events.push(ev);
            if finished {
                break;
            }
        }
        (report, events)
    }

    /// ⚠ All originals — the pre-existing export tests below keep their exact assertions
    /// through the widening to `PhotoId`, which is what shows copies changed nothing for
    /// ordinary photos.
    fn jobs(dir: &TempDir, names: &[&str]) -> Vec<ExportJob> {
        names
            .iter()
            .map(|n| ExportJob {
                id: PhotoId::original(dir.0.join(n)),
                name: None,
                edits: Edits::default(),
                capture: core_image::Capture::default(),
            })
            .collect()
    }

    fn ok_decode(
    ) -> impl Fn(&Path) -> Result<(ImageBuf, color::wb::Illuminant), String> + Send + 'static {
        |_: &Path| Ok((flat(8, 4), color::wb::Illuminant::default()))
    }

    /// Jobs carrying capture metadata, for the tokens that come from the catalog.
    fn jobs_with(dir: &TempDir, files: &[(&str, core_image::Capture)]) -> Vec<ExportJob> {
        files
            .iter()
            .map(|(n, c)| ExportJob {
                id: PhotoId::original(dir.0.join(n)),
                name: None,
                edits: Edits::default(),
                capture: c.clone(),
            })
            .collect()
    }

    /// 2026-08-27 10:00:00 as naive wall-clock seconds, on an X-T4.
    fn shot() -> core_image::Capture {
        core_image::Capture {
            taken: Some(1_787_824_800),
            camera: Some("FUJIFILM X-T4".to_string()),
            ..Default::default()
        }
    }

    fn templated(dir: &TempDir, template: &str) -> ExportOptions {
        ExportOptions {
            filename_template: template.to_string(),
            ..opts(dir, None)
        }
    }

    #[test]
    fn the_default_base_is_what_dest_for_has_always_produced() {
        // ⚠⚠ The regression guard for the whole feature: `dest_for` is now defined *through*
        // `dest_named`, so the two cannot drift — and the default template renders to the same
        // base, which is what makes an existing user's next export byte-identical.
        let dir = PathBuf::from("/out");
        for (copy, name) in [(0u32, None), (2, None), (3, Some("Warm"))] {
            let base = crate::filename_template::Template::default().render(&NameFacts {
                file_stem: "a",
                copy,
                copy_label: name,
                seq: 1,
                taken: None,
                camera: None,
            });
            assert_eq!(
                dest_named(&dir, &base, 0, ExportFormat::Jpeg),
                dest_for(
                    &dir,
                    Path::new("/in/a.RAF"),
                    copy,
                    name,
                    0,
                    ExportFormat::Jpeg
                ),
                "copy {copy}"
            );
        }
    }

    #[test]
    fn a_run_needs_both_a_destination_and_a_template_that_parses() {
        // ⚠⚠ Each half asserted with the *other* half valid, which is what makes this two
        // properties rather than one. A version that only looked at the destination would pass
        // any test that varied the destination alone — and would ship a dialog whose Export
        // button stays lit over a typo.
        let d = TempDir::new("can_export");
        let good = templated(&d, "{name}-{seq:3}");
        assert!(can_export(&good));
        assert!(
            !can_export(&ExportOptions {
                filename_template: "{camrea}".into(),
                ..good.clone()
            }),
            "an unknown token must stop the run"
        );
        assert!(
            !can_export(&ExportOptions {
                dest_dir: PathBuf::new(),
                ..good.clone()
            }),
            "an unset destination must stop the run"
        );
        assert!(!can_export(&ExportOptions {
            dest_dir: d.0.join("gone"),
            ..good
        }));
    }

    #[test]
    fn a_template_names_the_files_it_writes() {
        // End to end through the worker, because the tokens being right says nothing about the
        // worker reaching for them — the sabotage that skipped rendering entirely left every
        // `filename_template` unit test green.
        let d = TempDir::new("template_names");
        let (report, _) = run_export(
            jobs_with(&d, &[("a.RAF", shot()), ("b.RAF", shot())]),
            templated(&d, "{date}_{seq:3}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("2026-08-27_001.jpg").exists());
        assert!(d.0.join("2026-08-27_002.jpg").exists());
        assert!(!d.0.join("a.jpg").exists(), "the old naming must be gone");
    }

    #[test]
    fn seq_starts_at_one_and_follows_the_job_order() {
        // ⚠ 1-based and positional in the list the caller handed over, which is the order the
        // Library is showing. A 0-based `{seq}` would put `000` on the first photo of every run.
        let d = TempDir::new("template_seq");
        let (report, _) = run_export(
            jobs(&d, &["z.RAF", "a.RAF", "m.RAF"]),
            templated(&d, "{seq:2}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 3);
        for n in ["01.jpg", "02.jpg", "03.jpg"] {
            assert!(d.0.join(n).exists(), "{n} missing");
        }
        assert!(!d.0.join("00.jpg").exists(), "the run starts at 1");
    }

    #[test]
    fn a_photo_with_no_camera_leaves_no_dangling_separator() {
        // ⚠⚠ Ordinary rather than exotic: two of the author's own exported JPEGs carry no EXIF at
        // all, so a mixed selection hits both branches of the collapsing rule in one run.
        let d = TempDir::new("template_absent");
        let (report, _) = run_export(
            jobs_with(
                &d,
                &[("a.RAF", shot()), ("b.jpg", core_image::Capture::default())],
            ),
            templated(&d, "{name}-{camera}-{seq:2}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("a-FUJIFILM X-T4-01.jpg").exists());
        assert!(
            d.0.join("b-02.jpg").exists(),
            "the file with no camera keeps one separator, not two"
        );
        assert!(!d.0.join("b--02.jpg").exists());
    }

    #[test]
    fn a_template_that_will_not_parse_falls_back_to_the_old_naming() {
        // ⚠ The dialog refuses to start an export with an invalid template, so the only way here
        // is a hand-edited preset file. A folder named the old way beats no folder at all — and
        // beats a folder of files called `{camrea}`.
        let d = TempDir::new("template_bad");
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            templated(&d, "{camrea}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a.jpg").exists());
    }

    #[test]
    fn keep_both_appends_its_suffix_to_the_templated_name() {
        // ⚠⚠ The template is a **third** number and must not merge with `-N` (de-duplication) or
        // `_N` (copy identity). A template that drops `{name}` makes two photos want one name,
        // which is exactly when `-N` has to still work.
        let d = TempDir::new("template_dedup");
        let (report, _) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            templated(&d, "shoot"),
            vec![Decision::Resolve {
                collision: Collision::KeepBoth,
                apply_to_all: true,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("shoot.jpg").exists() && d.0.join("shoot-1.jpg").exists());
    }

    #[test]
    fn every_photo_produces_a_jpeg() {
        let d = TempDir::new("all");
        let (report, events) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![],
            ok_decode(),
        );

        assert_eq!(report.exported, 2);
        assert_eq!(report.skipped, 0);
        assert!(report.failed.is_empty() && !report.cancelled);
        assert!(d.0.join("a.jpg").exists() && d.0.join("b.jpg").exists());
        assert_eq!(events.first(), Some(&ExportEvent::Started { total: 2 }));
        let items = events
            .iter()
            .filter(|e| matches!(e, ExportEvent::Item { .. }))
            .count();
        assert_eq!(items, 2, "each photo reports progress exactly once");
    }

    #[test]
    fn two_sources_with_the_same_stem_collide_in_an_empty_folder() {
        // ⚠ `a.RAF` and `a.jpg` both map to `a.jpg`, so the check has to consider names
        // claimed *in this run*, not only files already on disk.
        //
        // Parameterised by format because the extension decides **which** pair collides:
        // exporting PNG, `a.RAF` and `a.jpg` no longer do — but `a.RAF` and `a.png` now do.
        // The property is format-independent; only the triggering pair moves.
        for (format, other) in [
            (ExportFormat::Jpeg, "a.jpg"),
            (ExportFormat::Png8, "a.png"),
            (ExportFormat::Tiff16, "a.tif"),
        ] {
            let d = TempDir::new(&format!("same_stem_{}", format.extension()));
            let (report, _) = run_export(
                jobs(&d, &["a.RAF", other]),
                ExportOptions {
                    format,
                    ..opts(&d, None)
                },
                vec![Decision::Resolve {
                    collision: Collision::KeepBoth,
                    apply_to_all: false,
                }],
                ok_decode(),
            );
            let ext = format.extension();
            assert_eq!(report.exported, 2, "{format:?}");
            assert!(
                d.0.join(format!("a.{ext}")).exists() && d.0.join(format!("a-1.{ext}")).exists(),
                "{format:?}"
            );
        }
    }

    #[test]
    fn the_destination_extension_follows_the_format() {
        let dir = PathBuf::from("/out");
        for f in ExportFormat::ALL {
            assert_eq!(
                dest_for(&dir, Path::new("/in/DSCF6509.RAF"), 0, None, 0, f),
                dir.join(format!("DSCF6509.{}", f.extension())),
                "{f:?}"
            );
            assert_eq!(
                dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 3, f),
                dir.join(format!("a-3.{}", f.extension())),
                "{f:?}"
            );
        }
    }

    #[test]
    fn skip_leaves_the_existing_file_untouched() {
        let d = TempDir::new("skip");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Skip,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!((report.exported, report.skipped), (0, 1));
        assert_eq!(std::fs::read(d.0.join("a.jpg")).unwrap(), b"original");
    }

    #[test]
    fn overwrite_replaces_it() {
        let d = TempDir::new("overwrite");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Overwrite,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert_ne!(std::fs::read(d.0.join("a.jpg")).unwrap(), b"original");
    }

    #[test]
    fn keep_both_finds_the_first_free_suffix() {
        let d = TempDir::new("keep_both");
        std::fs::write(d.0.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.0.join("a-1.jpg"), b"x").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::KeepBoth,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a-2.jpg").exists());
    }

    #[test]
    fn apply_to_all_stops_further_prompts_and_reuses_the_choice() {
        let d = TempDir::new("apply_all");
        for n in ["a.jpg", "b.jpg"] {
            std::fs::write(d.0.join(n), b"original").unwrap();
        }
        // Exactly one answer supplied: a second prompt would make `run_export` cancel, and
        // the assertions below would fail.
        let (report, events) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Skip,
                apply_to_all: true,
            }],
            ok_decode(),
        );
        assert_eq!((report.exported, report.skipped), (0, 2));
        let prompts = events
            .iter()
            .filter(|e| matches!(e, ExportEvent::Collision { .. }))
            .count();
        assert_eq!(prompts, 1, "the second collision must not prompt again");
        assert_eq!(std::fs::read(d.0.join("b.jpg")).unwrap(), b"original");
    }

    #[test]
    fn cancelling_at_a_prompt_ends_the_run() {
        let d = TempDir::new("cancel_prompt");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![Decision::Cancel],
            ok_decode(),
        );
        assert!(report.cancelled);
        assert_eq!(report.exported, 0);
        assert!(!d.0.join("b.jpg").exists(), "the rest must be untouched");
    }

    #[test]
    fn a_decode_failure_is_collected_and_the_batch_continues() {
        // One unreadable file must not cost the other photos their export.
        let d = TempDir::new("fail_one");
        let (report, _) = run_export(
            jobs(&d, &["bad.RAF", "good.RAF"]),
            opts(&d, None),
            vec![],
            |p: &Path| {
                if p.to_string_lossy().contains("bad") {
                    Err("no".to_string())
                } else {
                    Ok((flat(8, 4), color::wb::Illuminant::default()))
                }
            },
        );
        assert_eq!(report.exported, 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].0.ends_with("bad.RAF"));
        assert!(d.0.join("good.jpg").exists());
    }

    /// Drain events until the run reports itself finished.
    fn finished(worker: &ExportWorker) -> ExportReport {
        while let Ok(ev) = worker.events().recv() {
            if let ExportEvent::Finished(r) = ev {
                return r;
            }
        }
        panic!("the worker ended without a Finished event");
    }

    #[test]
    fn destination_readiness_distinguishes_unset_missing_and_usable() {
        // ⚠ A pure function of the path rather than logic inside the egui closure, which is
        // what makes "Export is disabled when the folder is gone" testable at all rather
        // than only verifiable by clicking. Same extraction as
        // `Settings::apply_delete_confirm` and `view_region`'s maths.
        assert!(
            !destination_ready(Path::new("")),
            "an unset destination is not ready"
        );
        let d = TempDir::new("dest_ready");
        assert!(destination_ready(&d.0), "a real directory is ready");
        assert!(
            !destination_ready(&d.0.join("gone")),
            "a path that does not exist is not ready — the unplugged-drive case"
        );
        // A *file* is not a destination, and this is the case a plain `exists()` gets wrong.
        let file = d.0.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(!destination_ready(&file), "a file is not a directory");
    }

    #[test]
    fn a_flag_already_set_stops_the_run_before_the_first_photo() {
        // libraw's `unpack` is not interruptible, so cancellation is checked between items.
        //
        // ⚠ The flag is passed in **already set**, not stored after spawning. An earlier
        // version called `worker.cancel()` after `spawn`, which races: `spawn` starts the
        // thread immediately, so whether the first photo was exported depended on who won.
        // It won on macOS every time and lost on Linux CI — `left: 1, right: 0`. If a test
        // needs the worker cancelled before it runs, the only way to say so without a race
        // is to hand it a flag that is already true.
        let d = TempDir::new("cancel_flag");
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            ok_decode(),
            Arc::new(AtomicBool::new(true)),
            pipeline::Resampler::default(),
        );
        let report = finished(&worker);
        assert!(report.cancelled);
        assert_eq!(report.exported, 0);
        assert!(!d.0.join("a.jpg").exists(), "nothing should be written");
    }

    #[test]
    fn cancel_reaches_the_worker_and_stops_the_next_photo() {
        // Covers the public `cancel()` — and with it that `spawn` hands the worker the *same*
        // flag the handle keeps, which a one-line store can still get wrong.
        //
        // A handshake, not a sleep: the decode announces that it has entered the first photo
        // and blocks until the test has cancelled, so the worker cannot race ahead. Two
        // resume permits are sent so that a *regression* — the worker wrongly starting the
        // second photo — fails an assertion instead of deadlocking the suite.
        let d = TempDir::new("cancel_api");
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            move |_: &Path| {
                let _ = started_tx.send(());
                let _ = resume_rx.recv();
                Ok((flat(8, 4), color::wb::Illuminant::default()))
            },
            Arc::new(AtomicBool::new(false)),
            pipeline::Resampler::default(),
        );

        started_rx.recv().expect("worker never reached a decode");
        worker.cancel();
        let _ = resume_tx.send(());
        let _ = resume_tx.send(());

        let report = finished(&worker);
        assert!(report.cancelled);
        // "Abandon, don't abort": the photo in flight finishes decoding but is discarded.
        assert_eq!(report.exported, 0);
        assert!(
            !d.0.join("a.jpg").exists(),
            "the cancelled photo was written"
        );
        assert!(
            !d.0.join("b.jpg").exists(),
            "the second photo must never start"
        );
    }

    #[test]
    fn cancelling_during_a_decode_does_not_write_that_photo() {
        // ⚠ The whole point of the post-decode check. libraw cannot be interrupted, so the
        // decode of the photo in flight completes — but the file the user cancelled must not
        // appear anyway. The decode fn flips the flag mid-photo, which is exactly what a
        // click on Cancel does while a decode is running.
        let d = TempDir::new("cancel_mid");
        let flag = Arc::new(AtomicBool::new(false));
        let in_decode = flag.clone();
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            move |_: &Path| {
                in_decode.store(true, Ordering::Relaxed);
                Ok((flat(8, 4), color::wb::Illuminant::default()))
            },
            flag,
            pipeline::Resampler::default(),
        );

        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            if let ExportEvent::Finished(r) = ev {
                report = r;
                break;
            }
        }
        assert!(report.cancelled);
        assert_eq!(
            report.exported, 0,
            "the cancelled photo must not be written"
        );
        assert!(!d.0.join("a.jpg").exists());
        assert!(!d.0.join("b.jpg").exists());
    }

    #[test]
    fn a_source_with_no_stem_still_produces_a_name() {
        // Defensive: `file_stem` is an Option, and a path ending in `..` has none.
        let dir = PathBuf::from("/out");
        let got = dest_for(&dir, Path::new("/in/.."), 0, None, 0, ExportFormat::Jpeg);
        assert!(got.starts_with("/out"));
        assert_eq!(got.extension().and_then(|e| e.to_str()), Some("jpg"));
    }

    #[test]
    fn an_options_file_from_before_the_space_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts *every* setting. The struct's **container-level** `#[serde(default)]` is what
        // prevents that — verified: removing a field-level attribute changes nothing here.
        //
        // ⚠ So what this test actually discriminates is the **value**: the default must be sRGB.
        // A wide space arriving by default would quietly change what every existing user's next
        // export contains, which is the failure worth guarding.
        let json = r#"{"dest_dir":"/tmp","quality":90,"max_edge":null,
                       "demosaic":"Fast","format":"Jpeg"}"#;
        let opts: ExportOptions = serde_json::from_str(json).expect("older options should load");
        assert_eq!(opts.space, color::OutputSpace::Srgb);
        assert_eq!(opts.quality, 90);
    }

    #[test]
    fn an_options_file_from_before_the_delivery_fields_still_loads() {
        // ⚠⚠ `SettingsStore::load()` is infallible, so one field that fails to parse silently
        // reverts *every* setting. The container-level `#[serde(default)]` is what prevents
        // that — this is the exact JSON an export written before M6.1 produced.
        //
        // ⚠ And what this really discriminates is the **values**: each of the five must default
        // to "do nothing", or an existing user's next export would silently gain a watermark, a
        // subfolder, or a different filename. Sabotaging `Default` on any one of them fails here.
        let json = r#"{"dest_dir":"/tmp","quality":90,"max_edge":null,
                       "demosaic":"Fast","format":"Jpeg","space":"Srgb"}"#;
        let opts: ExportOptions = serde_json::from_str(json).expect("older options should load");
        assert_eq!(opts.quality, 90, "the fields that were there still load");
        assert_eq!(
            opts.filename_template, DEFAULT_FILENAME_TEMPLATE,
            "an absent template means the naming every export already had"
        );
        assert_eq!(opts.metadata, ExportMetadata::default());
        assert_eq!(opts.watermark, Watermark::default());
        assert_eq!(opts.output_sharpening, OutputSharpening::default());
        assert_eq!(opts.delivery, Delivery::default());
        // Spelled out rather than left to `Default`, because "the default is inert" is the
        // property, and a `Default` that stopped being inert would still satisfy the four
        // comparisons above.
        assert!(opts.metadata.copyright.is_empty() && opts.metadata.creator.is_empty());
        assert_eq!(
            opts.metadata.scope,
            export::MetadataScope::All,
            "an older preset writes all metadata, the M6.2 default"
        );
        assert!(opts.watermark.text.is_empty(), "no watermark by default");
        // ⚠ And its *other* fields are not zero — `Watermark::default` is hand-written for
        // exactly that reason. A derived one would hand this options file a zero-height, fully
        // transparent mark, which the four `== Default::default()` comparisons above cannot see.
        assert_eq!((opts.watermark.size, opts.watermark.opacity), (3, 70));
        assert_eq!(opts.output_sharpening.amount, 0, "no output sharpening");
        assert!(opts.delivery.subfolder.is_empty(), "straight into dest_dir");
        assert_eq!(opts.delivery.then, PostExportAction::Nothing);
    }

    #[test]
    fn the_delivery_fields_round_trip() {
        // A preset carries `ExportOptions` whole, so anything that does not survive serde is a
        // setting the user appears to have made and did not.
        let opts = ExportOptions {
            filename_template: "{date}-{seq:3}".to_string(),
            metadata: ExportMetadata {
                copyright: "© 2026".into(),
                creator: "W".into(),
                caption: "c".into(),
                scope: export::MetadataScope::CopyrightOnly,
            },
            watermark: Watermark {
                text: "rustroom".into(),
                corner: WatermarkCorner::TopLeft,
                size: 7,
                opacity: 40,
            },
            output_sharpening: OutputSharpening { amount: 40 },
            delivery: Delivery {
                subfolder: "web".into(),
                then: PostExportAction::RevealInFileManager,
            },
            ..ExportOptions::default()
        };
        let round: ExportOptions =
            serde_json::from_str(&serde_json::to_string(&opts).unwrap()).unwrap();
        assert_eq!(round, opts);
    }

    #[test]
    fn the_banding_hint_appears_only_for_prophoto_in_eight_bit() {
        // ⚠ The condition is a pure function so it is testable without a dialog. It fires on
        // ProPhoto alone — see `OutputSpace::warns_in_8_bit` for why widening it would be wrong.
        use color::OutputSpace as S;
        use export::ExportFormat as F;
        assert!(banding_warning(S::ProPhoto, F::Jpeg).is_some());
        assert!(banding_warning(S::ProPhoto, F::Png8).is_some());
        assert!(banding_warning(S::ProPhoto, F::Tiff8).is_some());
        assert!(banding_warning(S::ProPhoto, F::Tiff16).is_none());
        assert!(banding_warning(S::ProPhoto, F::Png16).is_none());
        for f in F::ALL {
            for s in [S::Srgb, S::DisplayP3, S::AdobeRgb] {
                assert!(
                    banding_warning(s, f).is_none(),
                    "{} + {:?} should not warn",
                    s.label(),
                    f
                );
            }
        }
    }

    #[test]
    fn a_copy_exports_with_an_underscore_suffix() {
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223.jpg"),
            "the original keeps the plain stem"
        );
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                2,
                None,
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223_2.jpg")
        );
    }

    #[test]
    fn a_labelled_copy_exports_with_its_label_beside_the_file_name() {
        // ⚠⚠ M6.0 reverses what this test used to assert. The label used to *replace* the stem,
        // giving `Warm.jpg`, because the name **was** the export filename. Now the default
        // template is `{name}_{copy}`, so the export keeps the photograph it came from.
        //
        // ⚠ The old behaviour is not lost: it is the template `{copy}`, which
        // `the_copy_token_alone_reproduces_the_old_named_copy_export` pins.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                2,
                Some("Warm"),
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223_Warm.jpg")
        );
    }

    #[test]
    fn a_label_on_the_original_reaches_the_export_too() {
        // ⚠⚠ Also reversed by M6.0, and deliberately. Copy 0 could not be labelled at all before,
        // because a "name" on it would have been a second filename. A label is not a filename, so
        // the original can be "Colour" beside its "B&W" copy — and that must reach the export.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/a.RAF"),
                0,
                Some("Colour"),
                0,
                ExportFormat::Jpeg
            ),
            dir.join("a_Colour.jpg")
        );
    }

    #[test]
    fn two_copies_sharing_a_name_still_get_distinct_files() {
        // ⚠ Nothing stops two copies being called the same thing, so the name alone cannot be
        // relied on to be unique. The pre-existing `-N` de-duplication is what catches it, which
        // is the reason the two schemes were kept separate rather than merged.
        let dir = std::path::PathBuf::from("/out");
        let first = dest_for(
            &dir,
            Path::new("/in/a.RAF"),
            1,
            Some("Warm"),
            0,
            ExportFormat::Jpeg,
        );
        let second = dest_for(
            &dir,
            Path::new("/in/a.RAF"),
            2,
            Some("Warm"),
            1,
            ExportFormat::Jpeg,
        );
        assert_eq!(first, dir.join("a_Warm.jpg"));
        assert_eq!(second, dir.join("a_Warm-1.jpg"));
        assert_ne!(first, second);
    }

    #[test]
    fn the_two_numbering_schemes_compose_rather_than_collide() {
        // ⚠ `_N` is **identity** — which virtual copy, stable forever. `-N` is
        // **de-duplication** — the Nth file wanting this name in this folder, assigned at export
        // time. Different meanings, so different separators, and both can appear at once.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 2, None, 1, ExportFormat::Jpeg),
            dir.join("a_2-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 3, ExportFormat::Jpeg),
            dir.join("a-3.jpg"),
            "an original that collides is unchanged from before this feature"
        );
    }
}
