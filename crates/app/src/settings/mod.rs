//! Persisted application preferences.
//!
//! Deliberately tiny. Other settings (`show_filmstrip`, panel widths, last folder, export
//! defaults) move in when they are actually wanted, not speculatively — egui panel sizes in
//! particular are fiddly to save and restore, and that belongs in whatever spec needs them.

use crate::batch_export::ExportOptions;
use pipeline::Resampler;
use raw_decode::Demosaic;
use serde::{Deserialize, Serialize};

mod choices;
#[cfg(test)]
mod compat_tests;
mod confirm;
mod initial;
mod panel;
mod sizes;
mod store;
pub use choices::*;
pub use confirm::*;
pub use panel::*;
pub use sizes::*;
pub use store::*;

/// The Library views the top row shows beside Grid, which is always there (M10.18c).
///
/// ⚠ Container-level `#[serde(default)]` only — the rule of this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryViews {
    pub compare: bool,
    pub survey: bool,
    pub map: bool,
    pub people: bool,
}

impl Default for LibraryViews {
    fn default() -> Self {
        Self {
            compare: true,
            survey: true,
            map: true,
            people: true,
        }
    }
}

/// OpenStreetMap's standard tiles — the Map view's default server (M10.18b).
pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/// User preferences that survive a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which file formats the Library hides.
    ///
    /// ⚠⚠ **A standing preference, not per-folder state — which is why it lives here and not on
    /// `Library`.** "I shoot RAW+JPEG and only want to see the RAWs" is true of every folder, and
    /// having it reset on each switch made it something to re-set constantly. Reported in use.
    ///
    /// ⚠ The *rest* of `LibraryFilter` deliberately does **not** persist: a search string or a
    /// star selection is about the question you are asking right now, and a stale one that
    /// empties a newly opened folder reads as breakage.
    pub hidden_formats: crate::format_filter::FormatFilter,
    /// How the browser groups same-stem files — a RAW beside its JPEG.
    ///
    /// ⚠⚠ **`hidden_formats` above and `stacking.formats` are two format lists, and the order is
    /// filter-then-stack**: a hidden format cannot be a stack member, so hiding JPEG globally makes
    /// stacking a no-op. Both settings are legitimate; this is where that is written down.
    pub stacking: crate::stack::Stacking,
    /// Ask before deleting a preset. Cleared by "Don't ask again" in that dialog; restored
    /// from the Settings window.
    pub confirm_preset_delete: bool,
    /// The options the last export actually *used* — not what was last typed. See
    /// `RustroomApp::start_export`.
    pub export: ExportOptions,
    /// The options the last **import** actually used (M9.12) — written only when an import starts,
    /// `export`'s rule, so a cancelled dialog leaves them alone.
    pub import: crate::ingest::options::ImportOptions,
    /// The template the last **rename** used (M10.8d) — written only when a rename runs, so a
    /// cancelled dialog leaves it alone.
    pub rename_template: String,
    /// The HDR merge dialog's options (M10.17a) — written only when a merge starts, `export`'s rule.
    pub hdr: crate::hdr_merge::HdrSettings,
    /// The panorama merge dialog's options (M10.17b) — written only when a merge starts.
    pub panorama: crate::pano_merge::PanoSettings,
    /// The Print dialog's settings (M10.18e) — written only when a print starts, `export`'s rule.
    pub print: crate::print::layout::PrintSettings,
    /// Where a merge of downloaded Immich photos goes (2026-10-28) — never the Immich cache, which
    /// Clear… empties. Asked for by the first such merge; changed in Settings → Immich.
    pub immich_merge_folder: Option<std::path::PathBuf>,
    /// Tethered capture's options (M10.18a) — written only when a session starts.
    pub tether: crate::tether_options::TetherOptions,
    /// The Map view's tile server, a `{z}/{x}/{y}` template (M10.18b) — OpenStreetMap's by default.
    pub map_tile_url: String,
    /// Which views the Library's top row shows beside Grid (M10.18c, the author's request). A hidden view keeps
    /// its menu entry and shortcut.
    pub library_views: LibraryViews,
    /// Look for faces in local photos while the People view is open (M10.18c).
    pub find_faces: bool,
    /// The keyboard shortcuts the user changed, over the menu's defaults (M10.9b) — only the changes.
    pub shortcuts: Vec<crate::shortcuts::Rebinding>,
    /// Which network *Select Subject* uses (M9.13) — IS-Net (fast, the default) or BiRefNet_lite
    /// (high quality). The preview and the export both read it, so they pick the same cached map.
    pub ai_subject_model: ai_mask::SubjectModel,
    /// Ask before deleting an **export** preset.
    ///
    /// ⚠ A separate flag from `confirm_preset_delete`, not a shared one. A develop preset
    /// goes to a trash and can be restored with Ctrl+Z; an export preset is deleted outright.
    /// Suppressing the guard on the *recoverable* deletion must not disarm it on the
    /// irrecoverable one.
    pub confirm_export_preset_delete: bool,
    /// Demosaic tier for the **full-resolution editing buffer** — what the 1:1 view shows.
    ///
    /// ⚠ Independent of `export.demosaic`, and deliberately not mirrored from it. "The export
    /// preset's demosaic" is not one value — there are N named presets plus the last-used —
    /// and the same photo legitimately goes out as web-JPEG-fast and archive-TIFF-best.
    /// Inferring one would make the view's decode cost depend on distant state.
    ///
    /// Defaults to [`Demosaic::default()`], the one place that value lives.
    ///
    /// ⚠⚠ **That default moved `Fast` → `Better` at M8.4 (2026-09-10), which makes opening a
    /// photo ~2.4× slower and is a deliberate trade.** `Fast` was validated on hair-strand
    /// separation, and X-Trans demosaic does not fail on hair — it fails on foliage, where
    /// `Fast` smears thin twigs and mushes a distant treeline. ⚠ **Anyone whose settings file
    /// already holds `"Fast"` keeps it**, since a stored value beats a changed default; that is
    /// the trap recorded under the panel-section work, and here it is the safe direction.
    pub view_demosaic: Demosaic,
    /// Run the GPU-capable stages of the **global** tail on the graphics card.
    ///
    /// ⚠⚠ **Renamed from `gpu_hsl` at M8.3c, and the `alias` is a MIGRATION rather than
    /// decoration.** `Settings` carries a container-level `#[serde(default)]`, so without it an
    /// existing file holding `"gpu_hsl": true` would silently fall back to `false` — the user's
    /// choice discarded with nothing to notice it. The `EditGroups::effects` → `vignette` rename
    /// set that precedent and its alias test exists because the same trap was live there.
    ///
    /// ⚠ As of M8.3d the accelerated set is **HSL, colour grading and sharpening** — three
    /// consecutive tail stages, so they form one chain and one round trip.
    ///
    /// ⚠ The old name governed more than HSL as soon as sharpening joined it, and a setting that
    /// silently covers more than it claims is what the `Effects` edit group and the "Reset crop"
    /// hover text were each corrected for.
    ///
    /// ⚠⚠ **Defaults to `true` since 2026-09-18 — the user's call**, after M8.3b through M8.14 were
    /// each confirmed by use. It defaulted to `false` while the GPU path was young, on the
    /// reasoning that *"the CPU path is the oracle, and a wrong default would make a rendering
    /// difference look like a Rustroom bug rather than a setting."* That reasoning was right then
    /// and is spent now: the two paths agree to ~0.01 of an 8-bit level, and the GPU path is what
    /// every measurement in `docs/JOURNAL.md` since M8.3 describes.
    ///
    /// ⚠ **The global tail only.** A mask's own stages live inside `MaskStage`, which
    /// `gpu_pipeline::accelerate` cannot reach — and masked HSL is the case reported in use as
    /// slow.
    ///
    /// ⚠⚠ **NO field-level `#[serde(default)]`, and removing it was REQUIRED by the flip.** It used
    /// to be safe precisely because `bool::default()` and the intended default were the same
    /// `false`; the moment the intended default became `true` that attribute would have loaded
    /// **`false`** for every settings file written before today — shipping the feature disabled for
    /// everyone who already has one. That is the `auto_advance` trap verbatim, named in this very
    /// comment, and it was one word away from being repeated. The struct's container-level
    /// `#[serde(default)]` supplies the right value instead.
    #[serde(alias = "gpu_hsl")]
    pub gpu_tail: bool,
    /// Which kernel the three resampling stages read pixels with — straighten, perspective and
    /// lens correction.
    ///
    /// ⚠⚠ **One setting for the preview *and* the export, deliberately unlike `view_demosaic`.**
    /// That one has to be separate because the preview skips demosaic entirely, so there is no
    /// value it could share. The resampler runs identically in both, so a second setting would
    /// be a gratuitous way to make the preview lie about the file.
    ///
    /// ⚠ Not an `Edits` field: it is a rendering-quality preference rather than part of the
    /// photograph's description, so it must not travel with a paste, a preset, a sync or a
    /// sidecar. Two photographers can disagree about it and still exchange edits.
    ///
    /// Defaults to Catmull-Rom, chosen from measurement — see `pipeline::resample`.
    pub resampler: Resampler,
    /// Camera model → the `.dcp` profile chosen for it.
    ///
    /// ⚠ Keyed by **model**, not by photo: a profile is a calibration of the body. Per-photo is a
    /// recorded follow-up — it is what Lightroom does, in its Calibration panel — but it would put
    /// a *decode* parameter into `Edits`, which would then have to travel through the sidecar,
    /// presets, copy/paste, sync and the buffer-cache key. See the C4 spec.
    ///
    /// ⚠ `BTreeMap` rather than `HashMap` so the settings file has a stable order and a diff of it
    /// is readable.
    pub camera_profiles: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// The primaries of the display Rustroom is being viewed on.
    ///
    /// ⚠ Applied to the preview and to thumbnails only — **never to an export**, and never
    /// inside the pipeline. The transform lives in `render_engine::pack`, after the histogram
    /// is derived, so the histogram stays image-referred rather than describing your monitor.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already falls back to this struct's `Default`.
    pub display_space: color::OutputSpace,
    /// How the canvas (and the soft proof) brings colour sRGB cannot hold inside it (M6.4c).
    ///
    /// ⚠ The canvas only: an export carries its own in `ExportOptions::intent`, and thumbnails
    /// render with the default, because they are cached on disk and following a view toggle
    /// would leave a library of mixed thumbnails.
    pub canvas_intent: color::RenderingIntent,
    /// Which theme to render in. Dark by default: a light surround measurably changes how you
    /// judge exposure, so a photo editor defaults to dark and offers the rest.
    pub theme: ThemeChoice,
    /// The accent colour for active state.
    pub accent: Accent,
    /// How large the interface is drawn.
    pub ui_scale: UiScale,
    /// Which drag gesture pans the canvas. Scroll and two-finger swipe always pan.
    pub pan_drag: PanDrag,
    /// Which held mouse button makes a brush stroke erase for the duration of the gesture.
    ///
    /// ⚠ Whichever button [`Self::pan_drag`] uses always wins; see [`EraseDrag::button`].
    pub erase_drag: EraseDrag,
    /// What a drag in the tone curve's Parametric mode does. See [`ParametricDrag`].
    pub parametric_drag: ParametricDrag,
    /// What reorders the photos of an unfurled stack (M10.8c).
    pub stack_drag: StackDrag,
    /// Turn lens corrections on for photos that have no sidecar yet.
    ///
    /// ⚠ A **preference**, not an op-list default. `Edits::default()` stays strictly neutral so
    /// the CLI, the goldens and every existing sidecar are untouched; this only decides what a
    /// *fresh* photo starts with. Defaulting the op-list instead would dent that invariant in a
    /// way the golden harness structurally cannot see, because the goldens are synthetic buffers
    /// with no lens metadata and the stage stays inert there.
    ///
    /// ⚠ Consequence, accepted and documented rather than discovered: the GUI and the CLI then
    /// disagree on a fresh photo — the CLI renders neutral, the GUI renders corrected.
    pub auto_lens_correction: bool,
    /// What a freshly-opened photo's display rendering starts as.
    ///
    /// ⚠ A **preference**, not an op-list default — `auto_lens_correction`'s rule, for
    /// `auto_lens_correction`'s reason. `Edits::default()` stays strictly neutral, so the CLI,
    /// the goldens and every existing sidecar are untouched, and only a *fresh* photo opens
    /// rendered. Defaulting `Edits` instead would move every golden at once and destroy the
    /// harness's ability to say that this stage changed nothing else.
    ///
    /// ⚠ Consequence, stated rather than discovered: the GUI and the CLI disagree on a fresh
    /// photo — exactly as they already do about lens correction.
    pub default_rendering: edit_model::Rendering,
    /// Learn each film simulation's look from the camera's own JPEGs, and open a fresh RAW with it — Camera
    /// matching (M10.13d). ⚠ A separate switch rather than a third `default_rendering`, so a settings file
    /// written before it — which says `Camera` — gets it too.
    pub camera_match: bool,
    /// How the Library and the filmstrip order a folder.
    ///
    /// ⚠ Applied to the **scan**, not to `entries`, which is what keeps a file's copies adjacent
    /// to it and in ascending copy order under every ordering. See `RustroomApp::rebuild_entries`.
    pub library_sort: catalog::SortOrder,
    /// How many photos a batch export decodes at once.
    ///
    /// ⚠⚠ **A setting rather than a constant, because the binding constraint is RAM and nothing
    /// here can read how much is free.** Measured on an X-T4 at the default demosaic tier over
    /// eight files: **1.02 / 1.88 / 3.14 / 4.55x at widths 1 / 2 / 4 / 8**, holding **298 / 596 /
    /// 1192 / 2385 MiB** of decode buffers. The curve has no knee, so there is no one number that
    /// serves an 8 GB laptop and a 64 GB desktop — it is a straight memory-for-time trade.
    ///
    /// ⚠ Deriving it from the **core count** was rejected for a sharper reason than taste: it
    /// scales with the wrong thing. libraw's decode is single-threaded, so cores decide the
    /// ceiling and memory decides what is reachable.
    ///
    /// ⚠ `u32`, for the two reasons `library_cell` carries: `Settings` derives `Eq`, which no
    /// float satisfies, and a NaN width is **unrepresentable** rather than guarded. Read through
    /// [`Settings::export_width`], which repairs a hand-edited zero.
    pub export_concurrency: u32,
    /// The Library grid's cell size in whole points — thumbnails and their names scale with it.
    ///
    /// ⚠ `u32`, not `f32`, for two reasons: `Settings` derives `Eq`, which no float can satisfy;
    /// and it makes a NaN cell size **unrepresentable** rather than something to guard against —
    /// NaN defeats `clamp` entirely, since it fails every comparison and comes back unchanged.
    ///
    /// ⚠ Still clamped by [`Settings::clamped_cell`] rather than trusted: this is read from a
    /// file a user can edit, and a zero cell divides by zero when the column count is worked out.
    pub library_cell: u32,
    /// Survey's tile size in points.
    ///
    /// ⚠ Clamped by [`Settings::clamped_survey_cell`] rather than trusted — the file is
    /// hand-editable, `library_cell`'s rule.
    pub survey_cell: u32,
    /// The last custom crop ratio entered in the Geometry panel, as whole numbers W : H.
    ///
    /// ⚠ `u32`, for the two reasons `library_cell` carries: `Settings` derives `Eq`, which no
    /// float satisfies, and a NaN ratio is **unrepresentable** rather than guarded. Whole
    /// numbers cover the decimal cases — 2.35:1 is 235:100.
    ///
    /// ⚠ Read through [`Settings::custom_aspect_ratio`], which repairs a hand-edited zero:
    /// the file is user-editable and a zero divides by zero.
    ///
    /// ⚠ No field-level `#[serde(default)]` — the container-level one fills a missing field
    /// from this struct's `Default`, the `snapshot_strip_height` rule.
    pub custom_aspect: [u32; 2],
    /// Which Library panel sections are expanded. ⚠ Persisted because egui's own collapsing
    /// state does not survive a restart here — see [`PanelSections`].
    pub panel_sections: PanelSections,
    /// Where the Library rail, the edit panel and the filmstrip sit (M10.8b) — the View menu sets it.
    pub layout: crate::layout::Layout,
    /// Whether the tone curve is expanded — in the Tone section and in a mask's, one choice for
    /// both. Open by default; collapsing it is the author's way to keep "quite large" out of the
    /// way (2026-09-29), so it is **persisted**, for [`Self::panel_sections`]' reason.
    pub tone_curve_open: bool,
    /// The Develop snapshot strip's outer height in whole points.
    ///
    /// ⚠ `u32`, not `f32`, for the two reasons `library_cell` already carries: `Settings` derives
    /// `Eq`, which no float satisfies; and it makes a NaN height **unrepresentable** rather than
    /// something to guard against — NaN defeats `clamp` entirely, since it fails every comparison
    /// and comes back unchanged.
    ///
    /// ⚠ Still clamped by [`Settings::clamped_strip_height`] rather than trusted: this is read
    /// from a file a user can edit.
    ///
    /// ⚠ Written when the resize drag **settles**, never per frame — a resize runs at 60 Hz. The
    /// `apply_view_demosaic` rule.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already falls back to this struct's `Default`. A field-level one would use
    /// `u32`'s own default of `0` instead, silently giving every existing user a collapsed strip.
    pub snapshot_strip_height: u32,
    /// Sizes the user has dragged dialogs to, keyed by the dialog's id.
    ///
    /// ⚠ `[u32; 2]`, not floats: `Settings` derives `Eq`, which no float satisfies — the reasoning
    /// `library_cell` and `snapshot_strip_height` already carry.
    ///
    /// ⚠ `BTreeMap` so the file has a stable order and a diff of it is readable — the reason
    /// `camera_profiles` is one.
    ///
    /// ⚠ Persisted at all because **egui's own `Resize` state dies with the process**: this app
    /// implements no `eframe::App::save`. The panel-sections work established that a control whose
    /// state does not survive a restart is worth very little.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field comes from `Settings::default()` — an empty map.
    pub dialog_sizes: std::collections::BTreeMap<String, [u32; 2]>,
    /// The order the Library panel's sections are drawn in.
    ///
    /// ⚠⚠ **Never read directly — go through [`Settings::panel_order`]**, the method, which
    /// repairs it. The two share a name deliberately: the field is the raw stored value and the
    /// method the usable one, and no call site can confuse them because only the method returns a
    /// list with every section in it.
    ///
    /// ⚠ A `Vec` rather than `[PanelSection; 5]` precisely because `settings.json` is
    /// hand-editable and can hold any list at all.
    pub panel_order: Vec<PanelSection>,
    /// Which compositional overlay the crop rectangle carries.
    ///
    /// ⚠ Persisted for the reason `panel_sections` is: this app implements no
    /// `eframe::App::save`, so anything left to egui's own memory dies with the process — and a
    /// guide you have to re-pick on every launch is worth very little.
    ///
    /// ⚠ Defaults to [`CompositionGuide::Off`], and that is a *value* the tests assert rather
    /// than a mechanism. An overlay drawn over every photograph by default is a change nobody
    /// asked for.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already falls back to this struct's `Default`.
    pub composition_guide: crate::composition_guides::CompositionGuide,
    /// Write a `stem.xmp` sidecar beside every RAW when its rating, colour label, keywords or
    /// description change, so Lightroom and exiftool can read them.
    ///
    /// ⚠ Defaults to **true**: the whole point of the milestone is that a week of keywording
    /// does not stay locked inside a SQLite catalog no other application can read, and a
    /// default of `false` would make that opt-in for people who never find the setting.
    ///
    /// ⚠⚠ Writing **merges** into any existing sidecar rather than replacing it — see
    /// `xmp::write`. That is what makes it safe to leave on over a folder somebody has edited
    /// in Lightroom.
    ///
    /// ⚠ Turning it off does not stop *reading*: a foreign sidecar still imports on folder
    /// open. Turning it back on does not backfill either — the next mark change on each photo
    /// does.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field falls back to this struct's `Default` — which is `true`. A field-level one
    /// would use `bool`'s own default of `false` instead, silently disabling the feature for
    /// every existing user. The same trap `snapshot_strip_height` documents.
    pub write_xmp: bool,
    /// How much resolution a live gesture may trade for responsiveness.
    ///
    /// ⚠⚠ **Measured, not assumed:** on a large window a fitted photo renders at rung 3200
    /// (6.8 Mpx) at **137.9 ms mean over 131 instrumented drag frames — 7.2 fps** — where the fit
    /// rung renders the same photo in 35.2 ms. Dropping one rung is 4× fewer pixels.
    ///
    /// ⚠ Not an `Edits` field: it is a rendering-quality preference rather than part of the
    /// photograph's description, so it must not travel with a paste, a preset, a sync or a
    /// sidecar — `resampler`'s rule, for `resampler`'s reason.
    pub drag_quality: DragQuality,
    /// Move on to the next photo after a rating, a flag or a colour label.
    ///
    /// ⚠ Defaults to **true**: it is the whole point of M6.5c, and a culling feature nobody finds
    /// is a feature that did not ship. The correction case that usually argues for defaulting off
    /// is answered by [`crate::marks::MarkAction::advances`] instead — `0` and `U` do not advance,
    /// so fixing a mis-rating does not run away from you.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field falls back to this struct's `Default` — which is `true`. A field-level one
    /// would use `bool`'s own default of `false` instead, silently disabling the feature for every
    /// existing user. The same trap `write_xmp` and `snapshot_strip_height` both document.
    pub auto_advance: bool,
    /// The folders opened most recently, first first — the File menu's "Open recent".
    pub recent_folders: Vec<std::path::PathBuf>,
    /// Open what the Library last showed when Rustroom starts (M11.4).
    ///
    /// ⚠ Defaults to **true** through `Settings::default()` — no field-level `#[serde(default)]`, which would
    /// read `false` from every older file (`auto_advance`'s trap). `an_old_settings_file_reopens_and_welcomes`.
    pub reopen_last: bool,
    /// Show the welcome — what each part of the window is for — on the start page until it is ticked away
    /// (M11.4). ⚠ Defaults to **true**, the same trap as [`Self::reopen_last`].
    pub show_welcome: bool,
    /// What the Library showed last, mirrored by the bin's `place.rs` (M11.4); `None` when nothing was open.
    ///
    /// ⚠ Read leniently ([`crate::start::lenient`]): one unreadable place must not reset every setting. A missing
    /// field still takes the container-level default.
    #[serde(deserialize_with = "crate::start::lenient")]
    pub last_place: Option<crate::start::Place>,
    /// The Immich server's base URL, e.g. `https://photos.example.com`. Empty means unconfigured.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already reads as `String::default()`, which is the right "not configured".
    /// Whether the Immich integration exists at all.
    ///
    /// ⚠⚠ **Defaults to `true`, and that is the `gpu_tail` trap rather than laziness.** `Settings`
    /// carries a **container-level** `#[serde(default)]`, so a missing field takes whatever `Default`
    /// says — and defaulting this to `false` would make Immich **vanish on upgrade** for anyone
    /// already using it. `the_immich_gate_defaults_to_on` pins it so a later tidy-up cannot flip it.
    ///
    /// ⚠ A fresh user loses nothing to the `true` default: with no URL or key
    /// [`crate::immich_source::connection`] is `None` anyway, so every surface is already hidden.
    ///
    /// ⚠ Read **only** through `connection`, which is the one gate every Immich surface routes
    /// through — see that function for why there is no second condition anywhere.
    pub immich_enabled: bool,
    pub immich_url: String,
    /// The Immich API key.
    ///
    /// ⚠⚠ **In the OS keychain, not this file** (M10.15a): `SettingsStore` moves it there and keeps
    /// it in `settings.json` only where no keychain takes it — see `crate::secrets`. Read and written
    /// only through [`crate::immich_source::connection`] and [`crate::immich_source::set_connection`].
    ///
    /// ⚠ The Settings field's help text tells the user to paste a **read-only** key
    /// (`asset.read`, `asset.view`, `asset.download`, `album.read`). A key that cannot write
    /// cannot damage the library if this file leaks, which is what bounds the exposure.
    pub immich_key: String,
    /// A **separate** key, granted `asset.upload`, used only when an export uploads.
    ///
    /// ⚠⚠ **Two keys rather than one with the grant added, and the reason is structural.**
    /// Browsing, thumbnails and the rail run constantly and unattended; with one key every one of
    /// those requests would travel with a write-capable credential. Two keys make "browsing cannot
    /// write" a property of the **credential** rather than of our code being correct.
    ///
    /// ⚠ `asset.upload` is a grant *separate from* `asset.update` (verified against the 3.2.0
    /// permission enum, 168 values), so even this key **cannot modify or delete** existing assets.
    ///
    /// ⚠ Kept with [`Self::immich_key`] in the one keychain item (M10.15a).
    ///
    /// Empty means uploading is unavailable; see [`crate::immich_upload::availability`].
    pub immich_upload_key: String,
    /// Byte budget for the **local** thumbnail cache, in megabytes.
    ///
    /// ⚠⚠ No field-level `#[serde(default)]`. The struct carries a container-level one, so a
    /// missing field falls back to this struct's `Default` — 5120. A field-level one would use
    /// `u32`'s own default of **0**, turning the cache into a device that deletes every entry it
    /// writes. The same trap `write_xmp`, `auto_advance` and `gpu_tail` each document.
    ///
    /// ⚠ Megabytes rather than gigabytes so sub-gigabyte is expressible, and because the Settings
    /// row already displays MB.
    pub thumb_budget_mb: u32,
    /// Byte budget for the **Immich** thumbnail cache, in megabytes. Same trap, same reason.
    ///
    /// ⚠ A separate budget on a separate directory, so neither sweep can evict the other's
    /// entries.
    pub immich_thumb_budget_mb: u32,
    /// The soft limit on the downloaded Immich originals, in whole GB of 10⁹ bytes (M10.15b): past
    /// it, a dialog at startup offers to clear them. ⛔ Nothing is ever removed automatically.
    ///
    /// ⚠ No field-level `#[serde(default)]` — `thumb_budget_mb`'s trap: `u32`'s 0 would warn at
    /// every launch.
    pub immich_cache_limit_gb: u32,
    /// Whether startup warns when the downloads exceed [`Self::immich_cache_limit_gb`]. ⚠ No
    /// field-level default either: `bool`'s `false` would switch the warning off on upgrade.
    pub immich_cache_warn: bool,
    /// The soft limit on AI denoise's results, in whole GB of 10⁹ bytes (M10.16b): past it, a dialog at
    /// startup offers to clear them. ⛔ Nothing is ever removed automatically. ⚠ No field-level
    /// `#[serde(default)]`, for `immich_cache_limit_gb`'s reason.
    pub denoise_cache_limit_gb: u32,
    /// Whether startup warns when the results exceed [`Self::denoise_cache_limit_gb`] — no field-level
    /// default either, for `immich_cache_warn`'s.
    pub denoise_cache_warn: bool,
    /// AI denoise's network (M10.16b6) — FFDNet unless chosen otherwise; a file from before reads FFDNet
    /// (the container-level default).
    pub denoise_network: DenoiseNetwork,
    /// Demosaic tier for **rendered previews** (M9.14). `None` follows [`Self::view_demosaic`],
    /// so a preview matches what Develop shows at 1:1; a lower tier is a cost choice — on an
    /// X-T4 a frame decodes in 16 s at Better, 7 s at Fast, 3 s at Fastest (measured 2026-09-28).
    pub preview_demosaic: Option<Demosaic>,
    /// Byte budget for the rendered-preview cache, in megabytes. No field-level `serde(default)`,
    /// for `thumb_budget_mb`'s reason: `u32`'s own default is 0, a cache that deletes what it writes.
    pub preview_budget_mb: u32,
    /// Settings → Display's **Automatic** (2026-11-18, the default): the preview follows the monitor's own colours
    /// as `display_detect` finds them, rather than [`Self::display_space`]. No field-level default: the container's
    /// gives `true` — and a file from before that chose P3 or Adobe RGB by hand is put back to manual on load
    /// (`SettingsStore`), so nobody's choice is overridden.
    pub display_auto: bool,
    /// Settings → Behaviour's **Check for new versions** (piece 4): on by default — the author's call. Read through
    /// `update_check::may_check`, which also honours a build compiled without the check and a Flatpak.
    pub check_updates: bool,
    /// When the last update check FINISHED, in Unix seconds; `0` = never. A failed check counts too, so an offline
    /// machine is not asked again at every launch.
    pub update_checked_at: u64,
    /// The newest version a check found (`0.2.0`), kept so a second launch within the day still shows it.
    pub update_latest: Option<String>,
    /// The version the user hid with the status line's Hide — quiet until a newer one exists.
    pub update_dismissed: Option<String>,
}

/// How many photos a batch export decodes at once, out of the box.
///
/// ⚠ **4, measured rather than chosen**: 3.14x against sequential for ~1.2 GB of decode buffers on
/// a 26 Mpx camera. 8 reaches 4.55x for 2.4 GB, which is a fine trade on a desktop and a poor one
/// on a laptop — hence a setting, and hence a default that errs toward the laptop.
pub const DEFAULT_EXPORT_WIDTH: u32 = 4;

/// How many folders "Open recent" remembers.
pub const RECENT_FOLDERS_MAX: usize = 8;

impl Settings {
    /// Put `dir` at the front of the recent folders, once, and keep the list short.
    pub fn remember_folder(&mut self, dir: &std::path::Path) {
        self.recent_folders.retain(|p| p != dir);
        self.recent_folders.insert(0, dir.to_path_buf());
        self.recent_folders.truncate(RECENT_FOLDERS_MAX);
    }
}

/// The rename template before one was ever used: the capture date and a three-digit number, which
/// sorts by name in shooting order.
pub const DEFAULT_RENAME_TEMPLATE: &str = "{date}-{seq:3}";

impl Default for Settings {
    fn default() -> Self {
        Self {
            hidden_formats: crate::format_filter::FormatFilter::default(),
            // ⚠ `enabled: true` — see `Stacking::enabled` for why the default is not `false`.
            stacking: crate::stack::Stacking {
                enabled: true,
                ..Default::default()
            },
            immich_enabled: true,
            immich_url: String::new(),
            immich_key: String::new(),
            immich_upload_key: String::new(),
            thumb_budget_mb: 5120,
            immich_thumb_budget_mb: 5120,
            immich_cache_limit_gb: 10,
            immich_cache_warn: true,
            denoise_cache_limit_gb: 10,
            denoise_cache_warn: true,
            denoise_network: DenoiseNetwork::Ffdnet,
            preview_demosaic: None,
            preview_budget_mb: 10240,
            confirm_preset_delete: true,
            export: ExportOptions::default(),
            import: Default::default(),
            rename_template: DEFAULT_RENAME_TEMPLATE.to_string(),
            hdr: crate::hdr_merge::HdrSettings::default(),
            panorama: crate::pano_merge::PanoSettings::default(),
            print: crate::print::layout::PrintSettings::default(),
            immich_merge_folder: None,
            tether: Default::default(),
            map_tile_url: DEFAULT_TILE_URL.to_string(),
            library_views: LibraryViews::default(),
            find_faces: true,
            shortcuts: Vec::new(),
            ai_subject_model: Default::default(),
            confirm_export_preset_delete: true,
            view_demosaic: Demosaic::default(),
            gpu_tail: true,
            resampler: Resampler::default(),
            camera_profiles: std::collections::BTreeMap::new(),
            display_space: color::OutputSpace::default(),
            canvas_intent: color::RenderingIntent::default(),
            theme: ThemeChoice::default(),
            accent: Accent::default(),
            ui_scale: UiScale::default(),
            pan_drag: PanDrag::default(),
            erase_drag: EraseDrag::default(),
            parametric_drag: ParametricDrag::default(),
            stack_drag: StackDrag::default(),
            auto_lens_correction: true,
            default_rendering: edit_model::Rendering::Camera,
            camera_match: true,
            library_sort: catalog::SortOrder::default(),
            export_concurrency: DEFAULT_EXPORT_WIDTH,
            library_cell: DEFAULT_CELL,
            survey_cell: DEFAULT_SURVEY_CELL,
            custom_aspect: DEFAULT_CUSTOM_ASPECT,
            panel_sections: PanelSections::default(),
            layout: crate::layout::Layout::default(),
            tone_curve_open: true,
            snapshot_strip_height: DEFAULT_STRIP_HEIGHT,
            dialog_sizes: std::collections::BTreeMap::new(),
            panel_order: PanelSection::ALL.to_vec(),
            composition_guide: crate::composition_guides::CompositionGuide::Off,
            write_xmp: true,
            drag_quality: DragQuality::default(),
            auto_advance: true,
            recent_folders: Vec::new(),
            reopen_last: true,
            show_welcome: true,
            last_place: None,
            display_auto: true,
            check_updates: true,
            update_checked_at: 0,
            update_latest: None,
            update_dismissed: None,
        }
    }
}
