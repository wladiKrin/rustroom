//! What an export run is configured with: the jobs, the options, and delivery.
//!
//! ⚠ Split out of `batch_export.rs` on 2026-09-18 as a **pure move** — the `prefix_cache.rs`
//! precedent, with the test count unchanged and no golden moved. Every `⚠` below is the record of
//! a decision or a bug; **do not reword one.**

use super::*;
use crate::filename_template::Template;
use crate::photo_id::PhotoId;
use edit_model::Edits;
use export::ExportFormat;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// What the photo says about itself, from the catalog. ⚠ Carried for the same reason as
    /// `capture`: the worker has no catalog connection. Per **file**.
    pub description: catalog::db::Description,
    /// This copy's marks — the rating and label the export carries. Per **copy**.
    pub marks: catalog::db::Marks,
    /// The file's keyword paths, root first, from `xmp_sync::keyword_paths`.
    pub keywords: Vec<Vec<String>>,
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
    /// How colour `space` cannot hold is brought inside it (M6.4c). Saved in export presets; a
    /// preset written before it loads the default through the container-level `serde(default)`.
    /// ⚠ Inert for ProPhoto, which is the working space and maps nothing.
    pub intent: color::RenderingIntent,

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
    /// Where this export delivers what it renders.
    ///
    /// ⚠⚠ **One field rather than a flag beside a folder**, so "keep nothing and upload nothing" is
    /// **unrepresentable** instead of guarded. ⚠ Named for the *role*, never the service: `export`
    /// must stay able to lose the Immich integration with *"no mention of immich at all"*.
    ///
    /// ⚠ This replaced a day-old `upload_target: Option<UploadTarget>`. A preset written with that
    /// field still **loads** — the container-level `#[serde(default)]` sees to it — but comes back as
    /// `Files`, so the upload preference is re-ticked once. Pinned by
    /// `an_options_file_from_before_the_destination_still_loads`.
    pub destination: Destination,
    /// Which moment an uploaded photo is dated with. Ignored when nothing is uploaded.
    pub upload_date: UploadDate,
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

/// A destination an export can be delivered to besides the filesystem.
///
/// ⚠ One variant today, and that is not a placeholder — it is the only target that exists.
/// ⚠⚠ **Append variants, never insert**: this is serialised into settings and export presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadTarget {
    Immich,
}

/// Where an export run delivers what it renders.
///
/// ⚠⚠ **Append variants, never insert** — serialised into presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Destination {
    /// Write files and keep them. The default, and what every export did before this.
    #[default]
    Files,
    /// Deliver to the target only — **no local copy is kept**.
    Target(UploadTarget),
    /// Both.
    Both(UploadTarget),
    /// Neither tick set.
    ///
    /// ⚠⚠ **Appended, never inserted, and not the default.** Both ticks can be cleared, and
    /// [`can_export`] then refuses this while the footer note says why — chosen over refusing the
    /// click or auto-ticking the other box, because a checkbox that silently ignores a click, or one
    /// that changes a control the user did not touch, is worse than a dark button with a sentence
    /// beside it.
    ///
    /// ⚠ `keeps_files()` is false and `target()` is `None` here, so every existing caller already
    /// treats it as "write nothing, upload nothing" — which is right, and `can_export` is the only
    /// thing standing between it and a pointless run.
    Nowhere,
}

impl Destination {
    /// Whether a file survives the run, which is what decides if a folder is required.
    pub fn keeps_files(self) -> bool {
        matches!(self, Destination::Files | Destination::Both(_))
    }

    /// Whether this delivers anywhere at all. ⚠ The one thing `can_export` must refuse.
    pub fn is_nowhere(self) -> bool {
        matches!(self, Destination::Nowhere)
    }

    /// The upload target, when there is one.
    pub fn target(self) -> Option<UploadTarget> {
        match self {
            Destination::Files | Destination::Nowhere => None,
            Destination::Target(t) | Destination::Both(t) => Some(t),
        }
    }
}

/// Which moment an uploaded photo is dated with.
///
/// ⚠ The user's framing: there are *"valid arguments for all of these cases"*, so it is an option
/// rather than a rule. ⚠⚠ **Append variants, never insert.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UploadDate {
    /// The original photograph's own capture date. The default.
    #[default]
    Captured,
    /// When this photo's edits were last changed.
    Edited,
    /// When the upload happens.
    Uploaded,
}

impl UploadDate {
    pub const ALL: [UploadDate; 3] = [
        UploadDate::Captured,
        UploadDate::Edited,
        UploadDate::Uploaded,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UploadDate::Captured => "Original date",
            UploadDate::Edited => "Edit date",
            UploadDate::Uploaded => "Upload date",
        }
    }
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
            intent: color::RenderingIntent::default(),
            // ⚠ Not `String::default()`. The neutral filename template is `{name}`, which is
            // what reproduces the naming every export had before templating; an empty template
            // would fall back to the stem too, but only by accident rather than by rule.
            filename_template: DEFAULT_FILENAME_TEMPLATE.to_string(),
            metadata: ExportMetadata::default(),
            watermark: Watermark::default(),
            output_sharpening: OutputSharpening::default(),
            delivery: Delivery::default(),
            destination: Destination::Files,
            upload_date: UploadDate::Captured,
        }
    }
}

/// Whether a run can start at all: a usable destination, and a template that parses.
///
/// ⚠ A pure function rather than a condition inside the egui closure, for `destination_ready`'s
/// own reason — that is what makes "Export goes dark when the template is wrong" something a test
/// can see rather than something to verify by clicking. ⚠ And it is the place any *later*
/// pre-flight check belongs (a subfolder that will not sanitize, say), so the dialog keeps asking
/// one question.
pub fn can_export(opts: &ExportOptions, upload_ready: bool) -> bool {
    if opts.destination.is_nowhere() {
        return false;
    }
    let folder_ok = !opts.destination.keeps_files() || destination_ready(&opts.dest_dir);
    // ⚠⚠ A destination naming a target it cannot reach must not start: a preset saved as
    // "Immich only" would otherwise run with nowhere to put anything once the key was removed.
    let target_ok = opts.destination.target().is_none() || upload_ready;
    folder_ok && target_ok && Template::parse(&opts.filename_template).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// M6.4c: a preset written before the intent existed loads the default, and a non-default
    /// intent survives the round trip — ⚠ non-default, or a field serde skipped would pass.
    #[test]
    fn the_rendering_intent_round_trips_and_an_older_preset_loads_the_default() {
        let json = r#"{"dest_dir":"/tmp","quality":90,"space":"DisplayP3"}"#;
        let old: ExportOptions = serde_json::from_str(json).expect("older options should load");
        assert_eq!(old.intent, color::RenderingIntent::default());
        let other = color::RenderingIntent::ALL
            .into_iter()
            .find(|&i| i != color::RenderingIntent::default())
            .expect("two intents");
        let opts = ExportOptions {
            intent: other,
            ..ExportOptions::default()
        };
        let back: ExportOptions =
            serde_json::from_str(&serde_json::to_string(&opts).unwrap()).unwrap();
        assert_eq!(back.intent, other);
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

    /// ⚠ The default must keep files and upload nothing, so an existing user's next export is
    /// unchanged.
    #[test]
    fn the_default_destination_is_files_only() {
        let d = ExportOptions::default().destination;
        assert_eq!(d, Destination::Files);
        assert!(d.keeps_files());
        assert_eq!(d.target(), None);
    }

    /// ⚠⚠ "Upload nothing and keep nothing" must be **unrepresentable**, which is the whole reason
    /// this is one field rather than a flag beside a folder.
    #[test]
    fn every_destination_delivers_somewhere() {
        for d in [
            Destination::Files,
            Destination::Target(UploadTarget::Immich),
            Destination::Both(UploadTarget::Immich),
        ] {
            assert!(
                d.keeps_files() || d.target().is_some(),
                "{d:?} delivers nowhere"
            );
            assert!(!d.is_nowhere());
        }
        // ⚠ And the one state that deliberately delivers nowhere says so on every accessor.
        assert!(Destination::Nowhere.is_nowhere());
        assert!(!Destination::Nowhere.keeps_files());
        assert_eq!(Destination::Nowhere.target(), None);
        assert!(!Destination::Target(UploadTarget::Immich).keeps_files());
        assert!(Destination::Both(UploadTarget::Immich).keeps_files());
        assert_eq!(
            Destination::Both(UploadTarget::Immich).target(),
            Some(UploadTarget::Immich)
        );
    }

    #[test]
    fn a_destination_round_trips() {
        for d in [
            Destination::Files,
            Destination::Target(UploadTarget::Immich),
            Destination::Both(UploadTarget::Immich),
        ] {
            let o = ExportOptions {
                destination: d,
                ..ExportOptions::default()
            };
            let back: ExportOptions =
                serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
            assert_eq!(back.destination, d);
        }
    }

    /// ⚠⚠ An options file written before this field, **and one written with the day-old
    /// `upload_target`**, must both still load. The second loses only the upload preference — never
    /// the preset — which is the cost this replacement accepts rather than shimming.
    #[test]
    fn an_options_file_from_before_the_destination_still_loads() {
        let o: ExportOptions = serde_json::from_str(r#"{"dest_dir":"/tmp","quality":90}"#).unwrap();
        assert_eq!(o.destination, Destination::Files);
        assert_eq!(o.quality, 90);

        let stale: ExportOptions =
            serde_json::from_str(r#"{"quality":77,"upload_target":"Immich"}"#).unwrap();
        assert_eq!(stale.quality, 77, "the rest of the preset must survive");
        assert_eq!(
            stale.destination,
            Destination::Files,
            "the retired field is ignored, not honoured"
        );
    }

    #[test]
    fn the_default_upload_date_is_the_originals() {
        assert_eq!(ExportOptions::default().upload_date, UploadDate::Captured);
    }

    /// ⚠⚠ `can_export` must refuse a target the settings cannot serve, or a preset saved as
    /// "Immich only" starts a run with **nowhere to put anything** once the key is gone.
    #[test]
    fn can_export_needs_a_folder_only_when_files_are_kept() {
        let tmp = std::env::temp_dir();
        let with_dir = |d: Destination| ExportOptions {
            dest_dir: tmp.clone(),
            destination: d,
            ..ExportOptions::default()
        };
        let no_dir = |d: Destination| ExportOptions {
            dest_dir: std::path::PathBuf::new(),
            destination: d,
            ..ExportOptions::default()
        };

        assert!(can_export(&with_dir(Destination::Files), false));
        assert!(!can_export(&no_dir(Destination::Files), false));

        // Immich-only needs no folder at all …
        assert!(can_export(
            &no_dir(Destination::Target(UploadTarget::Immich)),
            true
        ));
        // … but does need uploading to be possible.
        assert!(!can_export(
            &no_dir(Destination::Target(UploadTarget::Immich)),
            false
        ));
        // Both needs the folder *and* the upload.
        assert!(can_export(
            &with_dir(Destination::Both(UploadTarget::Immich)),
            true
        ));
        assert!(!can_export(
            &with_dir(Destination::Both(UploadTarget::Immich)),
            false
        ));
        assert!(!can_export(
            &no_dir(Destination::Both(UploadTarget::Immich)),
            true
        ));
    }

    /// ⚠⚠ Unticking both destinations must darken Export, which is the whole reason `Nowhere` exists
    /// as a representable state rather than being prevented in the UI.
    #[test]
    fn nowhere_can_never_start_a_run() {
        let o = ExportOptions {
            dest_dir: std::env::temp_dir(),
            destination: Destination::Nowhere,
            ..ExportOptions::default()
        };
        // ⚠ Refused with a good folder *and* a working upload — so the refusal is about the
        // destination and nothing else.
        assert!(!can_export(&o, true));
        assert!(!can_export(&o, false));
        // ⚠ The control: the same options with a destination do start, or this proves nothing.
        assert!(can_export(
            &ExportOptions {
                destination: Destination::Files,
                ..o.clone()
            },
            false
        ));
    }

    #[test]
    fn nowhere_round_trips() {
        let o = ExportOptions {
            destination: Destination::Nowhere,
            ..ExportOptions::default()
        };
        let back: ExportOptions =
            serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back.destination, Destination::Nowhere);
        // ⚠ And the default is still Files, so an existing user is untouched by this addition.
        assert_eq!(ExportOptions::default().destination, Destination::Files);
    }
}
