//! Persisted application preferences.
//!
//! Deliberately tiny. Other settings (`show_filmstrip`, panel widths, last folder, export
//! defaults) move in when they are actually wanted, not speculatively — egui panel sizes in
//! particular are fiddly to save and restore, and that belongs in whatever spec needs them.

use crate::batch_export::ExportOptions;
use pipeline::Resampler;
use raw_decode::Demosaic;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which theme to render in.
///
/// ⚠ A mirror of `egui::ThemePreference`, which `CLAUDE.md` warns against on the grounds that
/// "a mapping that exists twice is one that will disagree" — the reason `Demosaic` lives in
/// `raw-decode` rather than being duplicated. Accepted here because the direction is forced:
/// this crate's **lib** must not depend on egui, since that separation is what keeps
/// `history`, `selection` and the rest testable without a windowing system. The mitigation is
/// that `theme.rs` maps it with three exhaustive match arms, so a new variant is a compile
/// error rather than a silent gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::System => "Follow system",
        }
    }
}

/// The colour used for *active* state — slider fill and handle, the open section, the primary
/// button.
///
/// ⚠ `Neutral` is not an absence: it is a grey, so "no accent" is a real choice rather than a
/// discarded one. Its cost is that active state falls back to lightness alone, which is
/// genuinely harder to read.
///
/// ⚠ **Amber is deliberately absent.** The objection is functional, not aesthetic: it sits on
/// the skin-tone and colour-temperature axis, which is precisely the judgement Temp and Tint
/// exist to serve, so a warm cast in the image becomes harder to see against it. It also
/// collides with amber's conventional meaning as a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Accent {
    Neutral,
    #[default]
    SteelBlue,
    Teal,
    Violet,
}

impl Accent {
    pub const ALL: [Accent; 4] = [
        Accent::Neutral,
        Accent::SteelBlue,
        Accent::Teal,
        Accent::Violet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Neutral => "Neutral (no accent)",
            Accent::SteelBlue => "Steel blue",
            Accent::Teal => "Teal",
            Accent::Violet => "Violet",
        }
    }
}

/// How a *drag* pans the canvas.
///
/// ⚠ Scroll and two-finger swipe always pan and are deliberately **not** configurable — they are
/// the universal route that works on every device, and making them optional would let a user
/// lock themselves out of panning entirely. Only the extra *drag* gesture is a setting.
///
/// ⚠ A closed three-value enum, not a keybinding system. The risk in this area is ballooning
/// into general rebinding; three values already cover every device. A mouse button is not a
/// modifier, which is why the set spans both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PanDrag {
    /// Middle-button drag. ⚠ Unavailable on an Apple Magic Mouse, which has no middle button —
    /// those users pan by swiping, which always works.
    #[default]
    Middle,
    /// Right-button drag. Free: nothing on the canvas uses secondary click or a context menu.
    Secondary,
    /// Hold Space and left-drag. The only option needing no extra hardware, and the
    /// Photoshop/Figma convention.
    SpaceLeft,
}

impl PanDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [PanDrag; 3] = [PanDrag::Middle, PanDrag::Secondary, PanDrag::SpaceLeft];

    /// How to describe this binding in the UI.
    ///
    /// ⚠ The toolbar hint must name the *configured* binding rather than saying "middle-drag"
    /// literally, or it becomes a lie the moment the setting changes. Same shape as
    /// `Mask::label()`.
    pub fn label(self) -> &'static str {
        match self {
            PanDrag::Middle => "middle-drag",
            PanDrag::Secondary => "right-drag",
            PanDrag::SpaceLeft => "Space+drag",
        }
    }
}

/// Which held mouse button makes a brush stroke erase, for the duration of the gesture.
///
/// ⚠⚠ **The pan binding always wins.** [`PanDrag`] is a deliberate choice about how to move around
/// the photograph, where this is a convenience — so a button bound to panning is never also an
/// erase, and the gesture is simply unavailable rather than both firing. That is the precedence
/// `tool_drag` already applies by refusing middle and secondary drags outright, so the pan gesture
/// cannot drag crop rectangles and mask handles as it passes over them.
///
/// ⚠ `Off` earns its place: an accidental right-drag erases part of a mask, and someone who finds
/// that destructive needs a way to turn it off that is not "bind panning to the same button",
/// which is exactly the kind of accident a setting should prevent rather than require.
///
/// ⚠ A closed enum, not a keybinding system — [`PanDrag`]'s reasoning, for its reason. A modifier
/// key was considered and left out: it would need its own keyboard-focus gating, which
/// [`PanDrag::SpaceLeft`] already documents as a trap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EraseDrag {
    /// Right-button drag. The default: nothing else on the canvas uses secondary click.
    #[default]
    Secondary,
    /// Middle-button drag. ⚠ Unavailable on an Apple Magic Mouse, which has no middle button.
    Middle,
    /// No erase gesture at all; the Erase checkbox is the only route.
    Off,
}

/// A mouse button, named here so this module needs no `egui` dependency.
///
/// ⚠ `crates/app/src/lib.rs` has no egui by rule — that separation is what keeps these types
/// testable without a window. The bin maps these onto `egui::PointerButton`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragButton {
    Secondary,
    Middle,
}

impl EraseDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [EraseDrag; 3] = [EraseDrag::Secondary, EraseDrag::Middle, EraseDrag::Off];

    /// How to describe this binding in the UI.
    ///
    /// ⚠ Named rather than spelled out at the call site, so a hint cannot go stale the moment the
    /// setting changes — [`PanDrag::label`]'s reason.
    pub fn label(self) -> &'static str {
        match self {
            EraseDrag::Secondary => "right-drag",
            EraseDrag::Middle => "middle-drag",
            EraseDrag::Off => "off",
        }
    }

    /// The button this binds to, or `None` when the gesture is off **or when panning has already
    /// claimed that button**.
    ///
    /// ⚠⚠ One function decides both, so "the pan binding wins" is written once. Two places
    /// deciding it is how a build appears in which a right-drag both pans and erases.
    pub fn button(self, pan: PanDrag) -> Option<DragButton> {
        let want = match self {
            EraseDrag::Secondary => DragButton::Secondary,
            EraseDrag::Middle => DragButton::Middle,
            EraseDrag::Off => return None,
        };
        let taken = match pan {
            PanDrag::Middle => Some(DragButton::Middle),
            PanDrag::Secondary => Some(DragButton::Secondary),
            PanDrag::SpaceLeft => None,
        };
        (Some(want) != taken).then_some(want)
    }
}

/// How much resolution a live gesture may trade for responsiveness.
///
/// ⚠⚠ **`Full` is zero rungs dropped, and that is the whole reason this is a setting rather than
/// a constant.** When the GPU port lands and the tail runs 5–15× faster, the feature is switched
/// off from Settings rather than removed from the code — **the setting is the deletion
/// mechanism.** It also lets a slower or faster machine pick its own point on the trade, which a
/// hardcoded rung cannot.
///
/// ⚠ `ALL`, `label` and `rungs_dropped` live **beside the type**, the rule `EDIT_GROUP_NAMES`,
/// `Tool::ALL` and `FlagSet::ALL` already follow, so the Settings UI cannot invent a second
/// ordering or a second mapping.
///
/// ⚠ `Fastest` differs from `Faster` **only when zoomed**: fitted, a two-rung drop from 3200
/// clamps at `fit_cap` and lands where one rung already did. Recorded so it is not later filed
/// as a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DragQuality {
    /// Render a drag at the same rung as an idle view — the behaviour before this existed.
    Full,
    #[default]
    Faster,
    Fastest,
}

impl DragQuality {
    pub const ALL: [DragQuality; 3] =
        [DragQuality::Full, DragQuality::Faster, DragQuality::Fastest];

    pub fn label(self) -> &'static str {
        match self {
            DragQuality::Full => "Full quality",
            DragQuality::Faster => "Faster",
            DragQuality::Fastest => "Fastest",
        }
    }

    /// How many rungs of the render ladder a live gesture drops.
    pub fn rungs_dropped(self) -> u32 {
        match self {
            DragQuality::Full => 0,
            DragQuality::Faster => 1,
            DragQuality::Fastest => 2,
        }
    }
}

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
    /// Ask before deleting a preset. Cleared by "Don't ask again" in that dialog; restored
    /// from the Settings window.
    pub confirm_preset_delete: bool,
    /// The options the last export actually *used* — not what was last typed. See
    /// `RustroomApp::start_export`.
    pub export: ExportOptions,
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
    /// ⚠⚠ **Defaults to `false`, deliberately.** The CPU path is the oracle, and a wrong default
    /// would make a rendering difference look like a Rustroom bug rather than a setting. The two
    /// agree to ~0.01 of an 8-bit level.
    ///
    /// ⚠ **The global tail only.** A mask's own stages live inside `MaskStage`, which
    /// `gpu_pipeline::accelerate` cannot reach — and masked HSL is the case reported in use as
    /// slow.
    ///
    /// ⚠ A **field-level** `#[serde(default)]` is safe here because `bool`'s default and the
    /// intended default are the same `false` — unlike `auto_advance`, where they differed and a
    /// field-level attribute would have shipped the feature disabled for everyone.
    #[serde(default, alias = "gpu_hsl")]
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
    /// Which theme to render in. Dark by default: a light surround measurably changes how you
    /// judge exposure, so a photo editor defaults to dark and offers the rest.
    pub theme: ThemeChoice,
    /// The accent colour for active state.
    pub accent: Accent,
    /// Which drag gesture pans the canvas. Scroll and two-finger swipe always pan.
    pub pan_drag: PanDrag,
    /// Which held mouse button makes a brush stroke erase for the duration of the gesture.
    ///
    /// ⚠ Whichever button [`Self::pan_drag`] uses always wins; see [`EraseDrag::button`].
    pub erase_drag: EraseDrag,
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
    /// How the Library and the filmstrip order a folder.
    ///
    /// ⚠ Applied to the **scan**, not to `entries`, which is what keeps a file's copies adjacent
    /// to it and in ascending copy order under every ordering. See `RustroomApp::rebuild_entries`.
    pub library_sort: catalog::SortOrder,
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
}

/// The Library grid's default cell size, and the size its type scale is relative to.
pub const DEFAULT_CELL: u32 = 168;
/// The smallest and largest cell the buttons will reach.
pub const CELL_RANGE: (u32, u32) = (108, 320);
/// The custom crop ratio's default, 5:4 — a print ratio none of the five preset chips offers.
pub const DEFAULT_CUSTOM_ASPECT: [u32; 2] = [5, 4];
/// Each side of a custom ratio, inclusive. 1000 lets 2.35:1 be written as 235:100.
pub const CUSTOM_ASPECT_RANGE: (u32, u32) = (1, 1000);

/// Survey's tile range, in points. ⚠ Wider at the top than the grid's: a survey of three photos
/// on a large monitor should be able to fill it, which is the whole complaint that put a size
/// control here.
pub const SURVEY_CELL_RANGE: (u32, u32) = (140, 900);

/// Survey's default tile size. Larger than [`DEFAULT_CELL`] because a survey holds a handful of
/// photographs where the grid holds hundreds.
pub const DEFAULT_SURVEY_CELL: u32 = 320;
/// How much one press of the grid-size buttons moves the cell.
pub const CELL_STEP: u32 = 28;

/// The snapshot strip's default outer height — about four rows at the compact metrics.
pub const DEFAULT_STRIP_HEIGHT: u32 = 110;
/// The smallest and largest the snapshot strip can be dragged to.
///
/// ⚠ The minimum is the header row alone, so someone who wants the space back can drag it away
/// once and have it stay gone. The maximum here is a ceiling on the *stored* value; the panel
/// additionally caps itself at half the live panel height, so a shrunken window clamps further.
pub const STRIP_HEIGHT_RANGE: (u32, u32) = (28, 400);

/// The largest a dialog may be *stored* as, in points.
///
/// ⚠ A ceiling on the stored value only. The dialog additionally clamps itself to 80% of the live
/// viewport, so a size chosen on a large monitor cannot strand it off-screen on a small one.
pub const MAX_DIALOG_EDGE: f32 = 2000.0;

impl Settings {
    /// The Library filter a session starts with: the persisted format filter, nothing else.
    ///
    /// ⚠⚠ **This is the read half of `hidden_formats`, and it did not exist.** The toggle wrote
    /// the setting on every change and `RustroomApp::new` built its filter from
    /// `LibraryFilter::default()`, so the file said "JPEGs hidden" and the app hid nothing —
    /// reported as *"the setting does not persist over sessions"*. The serde round-trip test
    /// beside it passed throughout: it could see the file and not the wiring.
    ///
    /// ⚠ The search text and the mark sets deliberately start empty — they are about the question
    /// being asked now, the filter bar's own rule; only the format is a standing preference.
    pub fn initial_filter(&self) -> crate::library_filter::LibraryFilter {
        crate::library_filter::LibraryFilter {
            format: self.hidden_formats.clone(),
            ..Default::default()
        }
    }

    /// The grid cell size in points, guaranteed usable.
    pub fn clamped_cell(&self) -> f32 {
        self.library_cell.clamp(CELL_RANGE.0, CELL_RANGE.1) as f32
    }

    /// The custom crop ratio as a pixel width / height, guaranteed finite and positive.
    pub fn custom_aspect_ratio(&self) -> f32 {
        let [w, h] = self.custom_aspect;
        let w = w.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        let h = h.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        w / h
    }

    /// Survey's tile size in points, guaranteed usable.
    ///
    /// ⚠ Its **own** value rather than the grid's, because the two are used for opposite things:
    /// the grid is for finding a photo among hundreds and wants small cells, a survey is for
    /// looking at a handful and wants large ones. Sharing the number would make sizing one
    /// resize the other.
    pub fn clamped_survey_cell(&self) -> f32 {
        self.survey_cell
            .clamp(SURVEY_CELL_RANGE.0, SURVEY_CELL_RANGE.1) as f32
    }

    /// The survey tile size a pinch of `factor` would give. [`Self::zoomed_cell`]'s rule, over
    /// the survey's own wider range.
    pub fn zoomed_survey_cell(&self, factor: f32) -> u32 {
        let now = self.clamped_survey_cell() as u32;
        if !factor.is_finite() || factor <= 0.0 {
            return now;
        }
        let mut next = (now as f32 * factor).round() as u32;
        if next == now {
            match factor.partial_cmp(&1.0) {
                Some(std::cmp::Ordering::Greater) => next = now.saturating_add(1),
                Some(std::cmp::Ordering::Less) => next = now.saturating_sub(1),
                _ => {}
            }
        }
        next.clamp(SURVEY_CELL_RANGE.0, SURVEY_CELL_RANGE.1)
    }

    /// The panel's section order, guaranteed to contain every section exactly once.
    ///
    /// ⚠⚠ **Repaired rather than trusted.** The file is user-editable, so the stored order can
    /// omit a section, list one twice, or be empty. Duplicates are dropped and anything missing is
    /// appended in default order, so **a section can never become unreachable** whatever the file
    /// says. That is `FormatFilter`'s hidden-by-exception robustness: the safe state is the one
    /// you get by default, not one you have to maintain.
    pub fn panel_order(&self) -> Vec<PanelSection> {
        let mut out: Vec<PanelSection> = Vec::with_capacity(PanelSection::ALL.len());
        for s in &self.panel_order {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        for s in PanelSection::ALL {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// The snapshot strip's height in points, guaranteed usable.
    pub fn clamped_strip_height(&self) -> f32 {
        self.snapshot_strip_height
            .clamp(STRIP_HEIGHT_RANGE.0, STRIP_HEIGHT_RANGE.1) as f32
    }

    /// The size dialog `id` should open at: what it was dragged to, else `default`, clamped.
    ///
    /// ⚠ Clamped rather than trusted — the settings file is user-editable, and a zero width would
    /// make a dialog unreachable. `clamped_cell`'s rule.
    pub fn dialog_size(&self, id: &str, min: (f32, f32), default: (f32, f32)) -> (f32, f32) {
        let (w, h) = match self.dialog_sizes.get(id) {
            Some([w, h]) => (*w as f32, *h as f32),
            None => default,
        };
        (
            w.clamp(min.0, MAX_DIALOG_EDGE),
            h.clamp(min.1, MAX_DIALOG_EDGE),
        )
    }

    /// Record the size dialog `id` is currently at.
    ///
    /// ⚠ In memory only — the **file** is written when the dialog closes, by `modals::draw`. A
    /// resize runs at 60 Hz and the settings file is not a place to write sixty times a second:
    /// the `apply_view_demosaic` rule.
    pub fn remember_dialog_size(&mut self, id: &str, size: (f32, f32)) {
        let v = [
            size.0.round().max(0.0) as u32,
            size.1.round().max(0.0) as u32,
        ];
        // ⚠ Only on a real change, so a dialog merely being open does not dirty the settings and
        // cause a needless file write when it closes.
        if self.dialog_sizes.get(id) != Some(&v) {
            self.dialog_sizes.insert(id.to_string(), v);
        }
    }

    /// The cell size after a pinch or ctrl+scroll of `factor`.
    ///
    /// ⚠⚠ **A step of at least one point is forced whenever a gesture is under way**, and that is
    /// the whole reason this is not a plain multiply. A trackpad pinch reports about 1.01 per
    /// frame; against a whole-point cell that rounds straight back to the same integer, so the
    /// grid would sit still until you pinched hard — indistinguishable from the gesture not being
    /// wired up at all.
    ///
    /// ⚠ egui's `zoom_delta` is a **multiplicative factor that is 1.0 when idle**, not a delta.
    /// Treating it as a delta collapses the size to nothing on every idle frame; this project has
    /// already paid for that once, in the canvas navigation work.
    pub fn zoomed_cell(&self, factor: f32) -> u32 {
        let now = self.clamped_cell() as u32;
        // ⚠ A non-finite or non-positive factor would otherwise produce a nonsense size through
        // the `as u32` cast, which saturates rather than erroring.
        if !factor.is_finite() || factor <= 0.0 {
            return now;
        }
        let mut next = (now as f32 * factor).round() as u32;
        if next == now {
            match factor.partial_cmp(&1.0) {
                Some(std::cmp::Ordering::Greater) => next = now.saturating_add(1),
                Some(std::cmp::Ordering::Less) => next = now.saturating_sub(1),
                _ => {}
            }
        }
        next.clamp(CELL_RANGE.0, CELL_RANGE.1)
    }

    /// The cell size one press of the bigger/smaller buttons would give.
    ///
    /// ⚠ Stepped from the *clamped* value, so a nonsense stored size is corrected by the first
    /// press rather than needing several to climb back into range.
    pub fn stepped_cell(&self, bigger: bool) -> u32 {
        let now = self.clamped_cell() as u32;
        let next = if bigger {
            now.saturating_add(CELL_STEP)
        } else {
            now.saturating_sub(CELL_STEP)
        };
        next.clamp(CELL_RANGE.0, CELL_RANGE.1)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hidden_formats: crate::format_filter::FormatFilter::default(),
            confirm_preset_delete: true,
            export: ExportOptions::default(),
            confirm_export_preset_delete: true,
            view_demosaic: Demosaic::default(),
            gpu_tail: false,
            resampler: Resampler::default(),
            camera_profiles: std::collections::BTreeMap::new(),
            display_space: color::OutputSpace::default(),
            theme: ThemeChoice::default(),
            accent: Accent::default(),
            pan_drag: PanDrag::default(),
            erase_drag: EraseDrag::default(),
            auto_lens_correction: true,
            default_rendering: edit_model::Rendering::Camera,
            library_sort: catalog::SortOrder::default(),
            library_cell: DEFAULT_CELL,
            survey_cell: DEFAULT_SURVEY_CELL,
            custom_aspect: DEFAULT_CUSTOM_ASPECT,
            panel_sections: PanelSections::default(),
            snapshot_strip_height: DEFAULT_STRIP_HEIGHT,
            dialog_sizes: std::collections::BTreeMap::new(),
            panel_order: PanelSection::ALL.to_vec(),
            composition_guide: crate::composition_guides::CompositionGuide::Off,
            write_xmp: true,
            drag_quality: DragQuality::default(),
            auto_advance: true,
        }
    }
}

/// One section of the Library panel.
///
/// ⚠ Exists so the panel's **order** can be a value rather than five literal calls. The bodies
/// stay exactly where they are; `collections_panel::show` matches this to each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelSection {
    Folders,
    Collections,
    Smart,
    Keywords,
    Dates,
}

impl PanelSection {
    /// Every section, in the order the panel shipped with.
    ///
    /// ⚠ Folders first because "where photos come from" is the outer concept, and All Photos —
    /// the whole catalog — is the natural top of it. That reasoning is `collections_panel`'s and
    /// is preserved here as the default rather than re-derived.
    pub const ALL: [PanelSection; 5] = [
        Self::Folders,
        Self::Collections,
        Self::Smart,
        Self::Keywords,
        Self::Dates,
    ];
}

/// Which of the Library panel's sections are expanded.
///
/// ⚠⚠ **Persisted here rather than left to egui**, because this app implements no
/// `eframe::App::save` — egui's own collapsing state dies with the process, so a collapsed
/// section would spring open on every launch and the control would be worth nothing.
///
/// ⚠⚠ **All CLOSED by default** (user's call, 2026-08-27). The first version defaulted them open,
/// on the argument that an existing user should lose no discoverability — but the point of the
/// control is that the panel does not show every option at once, and a default showing them all
/// makes it something you must set up before it helps. A collapsed header still names its
/// section, so nothing is hidden, only folded.
///
/// ⚠ `Default` is **derived**, which is correct only while every section defaults to *closed* —
/// `false` is `bool::default()`. Flip any of them and it must become a hand-written impl, the way
/// `RadialGradient::default` and `ColorGrading::default` already are. `..._still_loads` asserts
/// the value rather than the mechanism, so it catches either way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PanelSections {
    pub folders: bool,
    pub collections: bool,
    pub smart: bool,
    pub keywords: bool,
    pub dates: bool,
}

/// What a confirm dialog decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// Dismissed — nothing happens, and any "don't ask again" tick is discarded.
    Cancelled,
    /// Confirmed, carrying the "Don't ask again" tick.
    Confirmed { suppress_future: bool },
}

impl Settings {
    /// The edits a newly-opened photo starts with.
    ///
    /// ⚠⚠ **A photo with a sidecar keeps EXACTLY what it stored; the preferences seed a photo
    /// that has never been edited and nothing else.** Assigning them unconditionally is the bug
    /// this replaced — it overwrote the stored value on every open, so the Camera-rendering
    /// toggle reset to off on any photo that had ever been edited, and a stored lens correction
    /// silently reverted to all-off the same way. The old code's own comment claimed this
    /// behaviour while the line beneath it did the opposite.
    ///
    /// ⚠ Extracted as a pure function so the rule is testable without a window — the decision is
    /// the part worth testing, and the session glue is not.
    pub fn initial_edits(&self, stored: Option<edit_model::Edits>) -> edit_model::Edits {
        match stored {
            Some(e) => e,
            None => edit_model::Edits {
                lens_correction: self.initial_lens_correction(),
                rendering: self.initial_rendering(),
                ..edit_model::Edits::default()
            },
        }
    }

    /// What a **fresh** photo's rendering starts as.
    ///
    /// ⚠ `None` — *undecided* — when the preference is off, **not** `Some(Rendering::None)`:
    /// decided-off is a fact worth a sidecar, and seeding it would write a file for every photo
    /// ever opened under an off preference, where today none is written.
    pub fn initial_rendering(&self) -> Option<edit_model::Rendering> {
        (self.default_rendering != edit_model::Rendering::None).then_some(self.default_rendering)
    }

    /// What a **fresh** photo's lens corrections start as — `None`, undecided, when the
    /// preference is off, for `initial_rendering`'s reason.
    ///
    /// ⚠ No `has_sidecar` parameter any more: the "only when there is no sidecar" half of the
    /// rule lives in [`Self::initial_edits`], where it applies to every preference at once
    /// rather than being repeated per field and got wrong for the second one.
    pub fn initial_lens_correction(&self) -> Option<edit_model::LensCorrection> {
        self.auto_lens_correction
            .then(|| edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            })
    }

    /// The space the *renderer* should convert into.
    ///
    /// ⚠⚠ Every read of `display_space` in the render path must go through here. When the
    /// operating system is colour-matching the window for us — macOS, once the `CAMetalLayer`
    /// carries a colorspace — converting as well would be a **double conversion**: we would
    /// produce P3 and Core Animation would then treat it as sRGB and convert again, leaving the
    /// preview undersaturated and varying per monitor. Returning `Srgb` here makes that
    /// unrepresentable rather than something to remember.
    ///
    /// ⚠ The stored value is deliberately left alone, so a settings file shared with a Linux
    /// machine keeps a sensible choice for it.
    pub fn effective_display_space(&self, os_managed: bool) -> color::OutputSpace {
        if os_managed {
            color::OutputSpace::Srgb
        } else {
            self.display_space
        }
    }

    /// Fold a delete-confirm outcome into the settings. Returns whether to actually delete.
    ///
    /// ⚠ `suppress_future` is honoured **only** on `Confirmed`. Ticking "Don't ask again"
    /// and then cancelling must leave the guard armed — otherwise a user disarms a guard
    /// they just declined to use. Pure and separated from the egui code precisely so that
    /// rule is testable.
    pub fn apply_delete_confirm(&mut self, outcome: ConfirmOutcome) -> bool {
        apply_confirm(&mut self.confirm_preset_delete, outcome)
    }

    /// The same fold, for **export** presets and their own flag.
    pub fn apply_export_delete_confirm(&mut self, outcome: ConfirmOutcome) -> bool {
        apply_confirm(&mut self.confirm_export_preset_delete, outcome)
    }
}

/// The shared rule behind both confirm folds: `suppress_future` is honoured **only** on
/// `Confirmed`. One implementation because that rule is the load-bearing part, and two copies
/// of it would be two chances to get it wrong.
fn apply_confirm(flag: &mut bool, outcome: ConfirmOutcome) -> bool {
    match outcome {
        ConfirmOutcome::Cancelled => false,
        ConfirmOutcome::Confirmed { suppress_future } => {
            if suppress_future {
                *flag = false;
            }
            true
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings io: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings json: {0}")]
    Json(#[from] serde_json::Error),
}

/// The settings file. Path injected so tests never touch the real home directory.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `<platform config dir>/rustroom/settings.json`, beside the presets directory.
    ///
    /// ⚠ Not `~/.config` everywhere — see [`crate::presets::PresetStore::user_default`].
    pub fn user_default() -> Self {
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom")
            .join("settings.json");
        Self::new(path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the settings. **Infallible**: a missing or malformed file yields the defaults,
    /// because a corrupt settings file must never brick the app.
    pub fn load(&self) -> Settings {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write the settings, creating the directory if needed.
    ///
    /// Called on change rather than on quit: it is one flag in a tiny file, and quitting is
    /// not always graceful.
    pub fn save(&self, s: &Settings) -> Result<(), SettingsError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(s)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_carries_the_export_format() {
        let f = TempFile::new("format_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            export: ExportOptions {
                format: export::ExportFormat::Tiff16,
                ..ExportOptions::default()
            },
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().export.format, export::ExportFormat::Tiff16);
    }

    /// ⚠⚠ **The pan binding always wins, decided in one place.** Two places deciding it is how a
    /// build appears in which a right-drag both pans the photograph and erases into it.
    #[test]
    fn the_pan_binding_takes_the_button_from_the_erase_gesture() {
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::Middle),
            Some(DragButton::Secondary)
        );
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::SpaceLeft),
            Some(DragButton::Secondary)
        );
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::Secondary),
            None,
            "right-drag is bound to panning, so it must not also erase"
        );
        assert_eq!(
            EraseDrag::Middle.button(PanDrag::Middle),
            None,
            "middle-drag is bound to panning, so it must not also erase"
        );
        assert_eq!(
            EraseDrag::Middle.button(PanDrag::Secondary),
            Some(DragButton::Middle)
        );
    }

    /// ⚠ `Off` is off under every pan binding — a test of the conflict alone would pass against a
    /// version that only ever disabled the gesture by collision.
    #[test]
    fn the_erase_gesture_can_be_switched_off_outright() {
        for pan in PanDrag::ALL {
            assert_eq!(EraseDrag::Off.button(pan), None, "with pan {pan:?}");
        }
    }

    /// ⚠⚠ **`SettingsStore::load` is infallible**, so a field that fails to parse silently reverts
    /// **every** setting to its default — not just this one. A settings file written before this
    /// field existed must still load.
    #[test]
    fn a_settings_file_from_before_the_erase_binding_still_loads() {
        let json = r#"{"pan_drag":"Secondary","theme":"Light"}"#;
        let s: Settings = serde_json::from_str(json).expect("an older settings file must load");
        assert_eq!(s.pan_drag, PanDrag::Secondary, "the stored value survived");
        assert_eq!(
            s.erase_drag,
            EraseDrag::default(),
            "the absent field takes the struct's default"
        );
        // ⚠ And with right-drag panning, the default erase binding is correctly unavailable
        // rather than fighting it — the interaction, not just the two values.
        assert_eq!(s.erase_drag.button(s.pan_drag), None);
    }

    #[test]
    fn a_settings_file_holding_gpu_hsl_still_turns_the_gpu_on() {
        // ⚠⚠ **Renaming a serialised field is a MIGRATION, not a rename.** `Settings` carries a
        // container-level `#[serde(default)]`, so without `alias = "gpu_hsl"` an existing file
        // holding `"gpu_hsl": true` would silently fall back to `false` — the user's choice
        // discarded with nothing to notice it. The `EditGroups::effects` → `vignette` rename set
        // this precedent, and its alias test exists because the same trap was live there.
        let f = TempFile::new("gpu_hsl_alias");
        std::fs::write(&f.0, r#"{"gpu_hsl": true}"#).unwrap();
        assert!(
            SettingsStore::new(f.0.clone()).load().gpu_tail,
            "an old gpu_hsl must carry over to gpu_tail"
        );
        // ⚠ And the new name must work too — an alias that REPLACED the field name rather than
        // adding to it would pass the assertion above and break every current file.
        let g = TempFile::new("gpu_tail_name");
        std::fs::write(&g.0, r#"{"gpu_tail": true}"#).unwrap();
        assert!(SettingsStore::new(g.0.clone()).load().gpu_tail);
    }

    #[test]
    fn a_settings_file_from_before_the_export_format_still_loads() {
        // The forgiving-load property `ExportOptions::default()` exists for.
        let f = TempFile::new("format_old");
        std::fs::write(&f.0, r#"{"export":{"quality":80}}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert_eq!(loaded.export.quality, 80, "the old field must survive");
        assert_eq!(loaded.export.format, export::ExportFormat::default());
    }

    /// ⚠⚠ The wiring half, which the round-trip test below cannot see: a persisted format
    /// filter must reach the session's filter, or the file is written and never read — which is
    /// exactly how it shipped, and how it was reported.
    #[test]
    fn the_hidden_formats_reach_the_filter_at_startup() {
        let mut s = Settings::default();
        s.hidden_formats.toggle("jpg");
        let f = s.initial_filter();
        assert!(
            f.format.is_hidden("jpg"),
            "the persisted filter never reached the session"
        );
        assert!(
            f.text.is_empty() && f.stars.is_empty(),
            "only the format is a standing preference"
        );
    }

    #[test]
    fn the_hidden_formats_survive_a_restart() {
        // ⚠ A standing preference — "I shoot RAW+JPEG and only want the RAWs" is true of every
        // folder. Persisting it here is what stops it resetting on each source switch.
        // ⚠ This test alone was blind to the bug it was written for: it proves the *file*, and
        // `the_hidden_formats_reach_the_filter_at_startup` proves the *wiring*.
        let f = TempFile::new("hidden_formats_rt");
        let store = SettingsStore::new(f.0.clone());
        let mut s = Settings::default();
        s.hidden_formats.toggle("jpg");
        store.save(&s).unwrap();
        let loaded = store.load();
        assert!(loaded.hidden_formats.is_hidden("jpg"));
        assert!(!loaded.hidden_formats.is_hidden("raf"));
    }

    #[test]
    fn a_settings_file_from_before_the_format_filter_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting — which is why each new field needs this.
        let f = TempFile::new("hidden_formats_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert!(
            !loaded.hidden_formats.is_active(),
            "an absent filter must default to hiding nothing"
        );
    }

    #[test]
    fn drag_quality_maps_to_rungs() {
        // ⚠⚠ `Full` must be exactly zero. It is the off switch: when the GPU port lands, the
        // feature is disabled from Settings rather than removed from the code, and
        // `rung_below` returns its input unchanged at n == 0.
        assert_eq!(DragQuality::Full.rungs_dropped(), 0);
        assert_eq!(DragQuality::Faster.rungs_dropped(), 1);
        assert_eq!(DragQuality::Fastest.rungs_dropped(), 2);
        // Every variant is in ALL, so the Settings combo cannot silently omit one.
        assert_eq!(DragQuality::ALL.len(), 3);
        for q in DragQuality::ALL {
            assert!(!q.label().is_empty(), "{q:?} has no label");
        }
    }

    #[test]
    fn a_settings_file_from_before_drag_quality_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting — which is why each new field needs this.
        let f = TempFile::new("drag_quality_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(
            loaded.drag_quality,
            DragQuality::Faster,
            "an absent drag_quality must default to Faster"
        );
    }

    #[test]
    fn a_settings_file_from_before_the_composition_guide_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting — which is why each new field needs this.
        let f = TempFile::new("composition_guide_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        // ⚠ Asserted as a **value**. An overlay appearing over every photograph on upgrade is a
        // change nobody asked for, and only the value can say the default is the quiet one.
        assert_eq!(
            loaded.composition_guide,
            crate::composition_guides::CompositionGuide::Off,
            "an absent guide must default to Off, not to an overlay"
        );
    }

    #[test]
    fn a_settings_file_from_before_the_panel_sections_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting — which is why each new field needs this.
        let f = TempFile::new("panel_sections_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(
            loaded.panel_sections,
            PanelSections::default(),
            "an absent value must give the closed default, not a half-filled struct"
        );
        assert!(
            !loaded.panel_sections.folders,
            "and the default really is closed"
        );
    }

    #[test]
    fn a_partial_panel_sections_object_defaults_the_rest() {
        // ⚠ The container-level `#[serde(default)]`: a file naming only one section must still
        // parse — `SettingsStore::load` is infallible, so a failure here silently reverts *every*
        // setting rather than just this one.
        let f = TempFile::new("panel_sections_partial");
        std::fs::write(&f.0, r#"{"panel_sections":{"dates":true}}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(loaded.panel_sections.dates, "the named one is honoured");
        assert!(!loaded.panel_sections.folders, "the rest take the default");
        assert!(!loaded.panel_sections.smart);
    }

    #[test]
    fn settings_round_trip_carries_the_collapsed_sections() {
        let f = TempFile::new("panel_sections_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            panel_sections: PanelSections {
                keywords: true,
                dates: true,
                ..PanelSections::default()
            },
            ..Settings::default()
        };
        store.save(&s).unwrap();
        let got = store.load();
        assert!(got.panel_sections.keywords);
        assert!(got.panel_sections.dates);
        assert!(!got.panel_sections.collections);
    }

    #[test]
    fn settings_round_trip_carries_the_demosaic_choice() {
        let f = TempFile::new("demosaic_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            export: ExportOptions {
                demosaic: raw_decode::Demosaic::Best,
                ..ExportOptions::default()
            },
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().export.demosaic, raw_decode::Demosaic::Best);
    }

    #[test]
    fn a_settings_file_from_before_the_demosaic_choice_still_loads() {
        // The same forgiving-load property that `ExportOptions::default()` exists for.
        let f = TempFile::new("demosaic_old");
        std::fs::write(
            &f.0,
            r#"{"export":{"dest_dir":"/tmp/x","quality":80,"max_edge":null}}"#,
        )
        .unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert_eq!(loaded.export.quality, 80, "the old fields must survive");
        assert_eq!(loaded.export.demosaic, raw_decode::Demosaic::default());
    }

    #[test]
    fn a_settings_file_from_before_write_xmp_still_loads_and_it_defaults_to_on() {
        // ⚠ The forgiving-load property again, and here the *value* is what discriminates:
        // `bool`'s own default is `false`, so a field-level `#[serde(default)]` would silently
        // turn XMP writing off for every existing user while still parsing cleanly.
        let f = TempFile::new("write_xmp_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert!(
            loaded.write_xmp,
            "XMP writing must default to on for a settings file that predates it"
        );
        assert!(Settings::default().write_xmp);
    }

    #[test]
    fn the_export_delete_confirmation_defaults_to_on() {
        assert!(Settings::default().confirm_export_preset_delete);
    }

    #[test]
    fn the_two_delete_guards_are_independent() {
        // ⚠ The reason they are separate flags. A develop preset goes to a trash and can be
        // restored; an export preset is deleted outright. Disarming the guard on the
        // recoverable deletion must not disarm it on the irrecoverable one.
        let mut s = Settings::default();
        s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true,
        });
        assert!(!s.confirm_preset_delete);
        assert!(
            s.confirm_export_preset_delete,
            "suppressing the develop-preset guard disarmed the export one"
        );
    }

    #[test]
    fn cancelling_never_suppresses_the_export_confirmation() {
        let mut s = Settings::default();
        assert!(!s.apply_export_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_export_preset_delete);
        // Ticking the box and then cancelling must leave the guard armed.
        assert!(!s.apply_export_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_export_preset_delete);
    }

    #[test]
    fn confirming_with_the_box_ticked_suppresses_the_export_confirmation() {
        let mut s = Settings::default();
        assert!(s.apply_export_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true
        }));
        assert!(!s.confirm_export_preset_delete);
    }

    #[test]
    fn the_export_options_default_to_unset() {
        let s = Settings::default();
        assert!(s.export.dest_dir.as_os_str().is_empty());
        assert_eq!(s.export.quality, 92);
        assert_eq!(s.export.max_edge, None);
    }

    #[test]
    fn settings_round_trip_carries_the_export_options() {
        let f = TempFile::new("export_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            // Not the default, so the round trip actually carries it.
            hidden_formats: {
                let mut f = crate::format_filter::FormatFilter::default();
                f.toggle("png");
                f
            },
            confirm_preset_delete: false,
            confirm_export_preset_delete: false,
            // Listed explicitly rather than via `..Default::default()`: this literal being
            // exhaustive is what makes a new `Settings` field a compile error here, forcing
            // a decision about whether the round trip should carry it.
            view_demosaic: Demosaic::Better,
            gpu_tail: false,
            // Non-default, so a field re-defaulted on load cannot pass this round trip.
            resampler: Resampler::Lanczos3,
            display_space: color::OutputSpace::DisplayP3,
            camera_profiles: std::collections::BTreeMap::new(),
            theme: ThemeChoice::System,
            accent: Accent::Teal,
            // A non-default value on purpose: it proves the field is actually persisted
            // rather than being re-defaulted on load.
            pan_drag: PanDrag::Secondary,
            erase_drag: EraseDrag::Middle,
            // Both non-default, for the same reason: a field re-defaulted on load would look
            // like it round-tripped.
            library_sort: catalog::SortOrder::ModifiedDesc,
            library_cell: DEFAULT_CELL + CELL_STEP,
            survey_cell: DEFAULT_SURVEY_CELL,
            custom_aspect: DEFAULT_CUSTOM_ASPECT,
            // Likewise non-default, and inside `STRIP_HEIGHT_RANGE` so `clamped_strip_height`
            // cannot mask a field that failed to round-trip.
            snapshot_strip_height: DEFAULT_STRIP_HEIGHT + 20,
            // Non-default so a field re-defaulted on load cannot pass.
            dialog_sizes: std::collections::BTreeMap::from([("x".to_string(), [500u32, 300u32])]),
            // Non-default so a field re-defaulted on load cannot pass. ⚠ Stored *unrepaired* —
            // the repair lives only in the accessor, and this asserts it has not leaked into serde.
            panel_order: vec![PanelSection::Dates, PanelSection::Folders],
            // Non-default in both directions, so a re-defaulted field cannot pass.
            panel_sections: PanelSections {
                folders: true,
                ..PanelSections::default()
            },
            // Likewise non-default: the preference defaults to *on*, so persisting `false` is
            // what proves it round-trips rather than being re-defaulted.
            // Not the default, so the round trip actually carries it.
            default_rendering: edit_model::Rendering::None,
            auto_lens_correction: false,
            // Non-default so a field re-defaulted on load cannot pass.
            composition_guide: crate::composition_guides::CompositionGuide::Thirds,
            // Likewise non-default: it defaults to *on*, so persisting `false` is what proves
            // it round-trips rather than being re-defaulted.
            write_xmp: false,
            // Non-default for the same reason: `Faster` is the default, so only `Full` proves the
            // value survives a round trip rather than being re-defaulted underneath the test.
            drag_quality: DragQuality::Full,
            // Non-default for the same reason as its neighbours: the preference defaults to *on*,
            // so persisting `false` is what proves it round-trips rather than being re-defaulted.
            auto_advance: false,
            export: ExportOptions {
                dest_dir: std::path::PathBuf::from("/tmp/rustroom_out"),
                quality: 71,
                max_edge: Some(2048),
                ..ExportOptions::default()
            },
        };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
    }

    #[test]
    fn a_settings_file_from_before_export_options_still_loads() {
        // ⚠ The forgiving-load property, and the test most likely to be waved through as
        // obviously fine. Without `Default` on `ExportOptions` the `#[serde(default)]` on
        // `Settings` cannot fill the missing field, so every existing config would fail to
        // parse — and `load()` being infallible would turn that into *all* settings
        // silently reverting.
        let f = TempFile::new("export_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.export, ExportOptions::default());
    }

    #[test]
    fn a_settings_file_from_before_view_demosaic_still_loads() {
        // ⚠ Same property as the test above, for the same reason: `load()` is infallible, so
        // a field that fails to parse does not surface an error — it silently reverts *every*
        // setting to its default. A new field must therefore be fillable by `#[serde(default)]`.
        let f = TempFile::new("view_demosaic_old");
        std::fs::write(
            &f.0,
            r#"{"confirm_preset_delete": false, "confirm_export_preset_delete": false}"#,
        )
        .unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old fields must survive");
        assert!(!loaded.confirm_export_preset_delete);
        assert_eq!(
            loaded.view_demosaic,
            raw_decode::Demosaic::default(),
            "and the new one must be fillable from the one place the default lives"
        );
    }

    #[test]
    fn a_settings_file_from_before_custom_aspect_still_loads() {
        // ⚠ `load()` is infallible, so a field that fails to parse silently reverts *every*
        // setting. The container-level `#[serde(default)]` is what fills the new field.
        let f = TempFile::new("custom_aspect_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old fields must survive");
        assert_eq!(loaded.custom_aspect, DEFAULT_CUSTOM_ASPECT);
    }

    #[test]
    fn a_hand_edited_custom_aspect_is_repaired_rather_than_dividing_by_zero() {
        let mut s = Settings {
            custom_aspect: [0, 7],
            ..Default::default()
        };
        let r = s.custom_aspect_ratio();
        assert!(r.is_finite() && r > 0.0, "{r}");
        assert!(
            (r - 1.0 / 7.0).abs() < 1e-6,
            "a zero repairs to the minimum: {r}"
        );
        s.custom_aspect = [5, 4];
        assert!((s.custom_aspect_ratio() - 1.25).abs() < 1e-6);
        s.custom_aspect = [50_000, 1];
        assert!(
            (s.custom_aspect_ratio() - 1000.0).abs() < 1e-6,
            "clamped at the top"
        );
    }

    #[test]
    fn the_view_tier_round_trips_through_a_file() {
        let f = TempFile::new("view_demosaic_rt");
        let store = SettingsStore::new(f.0.clone());
        // ⚠ Struct-update syntax, not `let mut s = Settings::default();` followed by a field
        // assignment — clippy's `field_reassign_with_default` is warn-by-default and CI runs
        // with `-D warnings`.
        let s = Settings {
            view_demosaic: raw_decode::Demosaic::Best,
            ..Default::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().view_demosaic, raw_decode::Demosaic::Best);
    }

    /// ⚠ `load()` is infallible, so an unparseable field does not surface an error — it silently
    /// reverts *every* setting. A new field must be fillable by `#[serde(default)]`, which is
    /// what the container-level attribute on `Settings` is for.
    ///
    /// ⚠ The assertion is on the **value**, not merely that the load succeeded: a resampler
    /// arriving as bilinear would quietly hand every existing user the old, softer kernel and
    /// the feature would look like it had never shipped.
    #[test]
    fn a_settings_file_from_before_the_resampler_still_loads_and_gets_catmull_rom() {
        let f = TempFile::new("resampler_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.resampler, Resampler::CatmullRom);
    }

    #[test]
    fn the_resampler_round_trips_through_a_file() {
        let f = TempFile::new("resampler_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            resampler: Resampler::Bilinear,
            ..Default::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().resampler, Resampler::Bilinear);
    }

    /// A unique temp file path for one test, removed on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_settings_{tag}.json"));
            let _ = std::fs::remove_file(&p);
            Self(p)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn the_delete_confirm_defaults_to_on() {
        assert!(Settings::default().confirm_preset_delete);
    }

    #[test]
    fn an_absent_file_loads_defaults() {
        let f = TempFile::new("absent");
        assert_eq!(SettingsStore::new(f.0.clone()).load(), Settings::default());
    }

    #[test]
    fn a_malformed_file_loads_defaults_rather_than_failing() {
        // ⚠ A corrupt settings file must never brick the app — the same forgiving posture
        // the JSON sidecars already take.
        let f = TempFile::new("malformed");
        std::fs::write(&f.0, "{ this is not json").unwrap();
        assert_eq!(SettingsStore::new(f.0.clone()).load(), Settings::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let f = TempFile::new("roundtrip");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            confirm_preset_delete: false,
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        // Forward compatibility: a file written by a newer build must still load.
        let f = TempFile::new("unknown");
        std::fs::write(
            &f.0,
            r#"{"confirm_preset_delete": false, "future_flag": 7}"#,
        )
        .unwrap();
        assert!(!SettingsStore::new(f.0.clone()).load().confirm_preset_delete);

        let g = TempFile::new("emptyobj");
        std::fs::write(&g.0, "{}").unwrap();
        assert_eq!(SettingsStore::new(g.0.clone()).load(), Settings::default());
    }

    #[test]
    fn cancelling_never_suppresses_the_confirmation() {
        // ⚠ The trap. Tick "Don't ask again", think better of the deletion, hit Cancel —
        // and the guard you just declined to use must still be armed next time.
        let mut s = Settings::default();
        assert!(!s.apply_delete_confirm(ConfirmOutcome::Cancelled));
        assert!(s.confirm_preset_delete, "cancel disarmed the confirmation");
    }

    #[test]
    fn confirming_with_the_box_ticked_suppresses_future_confirmations() {
        let mut s = Settings::default();
        assert!(s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: true
        }));
        assert!(!s.confirm_preset_delete);
    }

    #[test]
    fn confirming_without_the_box_leaves_the_setting_alone() {
        let mut s = Settings::default();
        assert!(s.apply_delete_confirm(ConfirmOutcome::Confirmed {
            suppress_future: false
        }));
        assert!(s.confirm_preset_delete);
    }

    #[test]
    fn a_settings_file_from_before_the_appearance_settings_still_loads() {
        // ⚠ The guard every new `Settings` field needs. `load()` is infallible, so a field
        // that fails to parse does not surface an error — it silently reverts *every* setting.
        let f = TempFile::new("appearance_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.theme, ThemeChoice::Dark, "dark is the default");
        assert_eq!(loaded.accent, Accent::SteelBlue);
    }

    #[test]
    fn the_appearance_settings_round_trip() {
        let f = TempFile::new("appearance_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            theme: ThemeChoice::Light,
            accent: Accent::Violet,
            ..Default::default()
        };
        store.save(&s).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert_eq!(loaded.accent, Accent::Violet);
    }

    #[test]
    fn every_accent_and_theme_is_in_its_table() {
        // The dialog and the colour lookup both iterate `ALL`; a variant missing from it is
        // invisible in the UI rather than a compile error, so it is worth an explicit check.
        assert_eq!(Accent::ALL.len(), 4);
        assert_eq!(ThemeChoice::ALL.len(), 3);
        for a in Accent::ALL {
            assert!(!a.label().is_empty());
        }
        for t in ThemeChoice::ALL {
            assert!(!t.label().is_empty());
        }
    }

    #[test]
    fn every_pan_drag_has_a_label() {
        // A table over every variant so a fourth cannot ship showing nothing in the toolbar
        // hint. The same shape `every_demosaic_tier_has_a_cost_hint` and `Mask::label` use.
        for b in PanDrag::ALL {
            assert!(!b.label().is_empty(), "{b:?} has no label");
        }
    }

    #[test]
    fn the_pan_drag_labels_are_distinct() {
        // Two variants sharing a label would make the hint ambiguous about which is active.
        for (i, a) in PanDrag::ALL.iter().enumerate() {
            for b in &PanDrag::ALL[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
    }

    #[test]
    fn the_default_pan_drag_is_the_middle_button() {
        assert_eq!(PanDrag::default(), PanDrag::Middle);
        assert_eq!(Settings::default().pan_drag, PanDrag::Middle);
    }

    #[test]
    fn a_settings_file_from_before_pan_drag_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts EVERY setting rather than erroring. That is why this test exists and is not
        // a nicety — the same trap the view-demosaic work documented.
        //
        // ⚠ `confirm_preset_delete` is written as `false` deliberately: it *defaults* to true,
        // so a whole-file parse failure is visible as it flipping back. Asserting a field whose
        // written value equalled its default would prove nothing.
        let f = TempFile::new("pan_drag_compat");
        std::fs::write(
            &f.0,
            r#"{"confirm_preset_delete":false,"confirm_export_preset_delete":false}"#,
        )
        .unwrap();
        let s = SettingsStore::new(f.0.clone()).load();
        assert_eq!(s.pan_drag, PanDrag::Middle, "the new field must default");
        assert!(
            !s.confirm_preset_delete,
            "an old field must survive — if this flipped to true, the whole file failed to parse"
        );
    }

    #[test]
    fn a_settings_file_from_before_the_display_space_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts *every* setting. The container-level `#[serde(default)]` is what prevents
        // that — so what this test actually discriminates is the **value**: the default must be
        // sRGB, because a wide space arriving by default would change what every existing user
        // sees without them asking for it.
        let json = r#"{"confirm_preset_delete":true}"#;
        let s: Settings = serde_json::from_str(json).expect("older settings should load");
        assert_eq!(s.display_space, color::OutputSpace::Srgb);
        assert!(s.confirm_preset_delete);
    }

    #[test]
    fn the_os_managing_colour_overrides_the_manual_setting() {
        // ⚠⚠ The property that makes double conversion unrepresentable. If both were live we
        // would convert sRGB→P3 ourselves and Core Animation would then treat *that* as sRGB and
        // convert again — visibly undersaturated, and varying per monitor.
        let mut s = Settings::default();
        for space in color::OutputSpace::DISPLAYS {
            s.display_space = space;
            assert_eq!(
                s.effective_display_space(true),
                color::OutputSpace::Srgb,
                "{} should be ignored while the OS is managing colour",
                space.label()
            );
        }
    }

    #[test]
    fn without_the_os_managing_the_manual_setting_is_used() {
        // ⚠ The fallback, and it is not a formality: if tagging the layer ever fails, this is
        // what stops macOS being left *uncorrected* — worse than what piece D shipped.
        let mut s = Settings::default();
        for space in color::OutputSpace::DISPLAYS {
            s.display_space = space;
            assert_eq!(s.effective_display_space(false), space);
        }
    }

    #[test]
    fn the_stored_setting_survives_the_os_taking_over() {
        // ⚠ `effective_display_space` must not *write*. A settings file shared between a Mac and
        // a Linux machine has to keep a sensible value for the latter.
        let s = Settings {
            display_space: color::OutputSpace::AdobeRgb,
            ..Default::default()
        };
        let _ = s.effective_display_space(true);
        assert_eq!(s.display_space, color::OutputSpace::AdobeRgb);
    }

    #[test]
    fn a_settings_file_from_before_camera_profiles_still_loads() {
        let json = r#"{"confirm_preset_delete":true}"#;
        let s: Settings = serde_json::from_str(json).expect("older settings should load");
        assert!(s.camera_profiles.is_empty());
    }

    #[test]
    fn the_camera_profile_mapping_round_trips() {
        let mut s = Settings::default();
        s.camera_profiles
            .insert("X-T4".into(), std::path::PathBuf::from("/p/x-t4.dcp"));
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(
            back.camera_profiles.get("X-T4").and_then(|p| p.to_str()),
            Some("/p/x-t4.dcp")
        );
    }

    #[test]
    fn a_settings_file_from_before_auto_lens_correction_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts *every* setting. `#[serde(default)]` is what stands in the way.
        let s: Settings = serde_json::from_str(r#"{"confirm_preset_delete":false}"#).unwrap();
        assert!(!s.confirm_preset_delete);
        assert!(
            s.auto_lens_correction,
            "the new field must default to on, not to false"
        );
    }

    /// ⚠⚠ **The default is `Camera`, and the value is what is asserted rather than the
    /// mechanism.** A rendering off by default would leave every RAW opening flat — the complaint
    /// this milestone exists to answer — and nothing else in the suite can see that, because
    /// `Edits::default()` is deliberately still `None` and every golden depends on it.
    #[test]
    fn a_fresh_photo_opens_rendered_and_corrected() {
        let s = Settings::default();
        assert_eq!(
            s.default_rendering,
            edit_model::Rendering::Camera,
            "a fresh RAW must not open colorimetric-only"
        );
        let fresh = s.initial_edits(None);
        assert_eq!(fresh.rendering, Some(edit_model::Rendering::Camera));
        assert!(fresh.applied_lens_correction().is_active(), "and corrected");
    }

    /// ⚠⚠ **THE BUG THIS FUNCTION EXISTS FOR, and it was reported from using the app: the
    /// Camera-rendering toggle "always reset to off" after switching photos.** The old seam
    /// assigned both preferences *unconditionally*, so every photo that had ever been edited had
    /// its stored rendering overwritten with `None` and its stored lens corrections with
    /// all-off — on every single open.
    ///
    /// ⚠ Both fields are asserted, because a fix that restored only the one that was reported
    /// would leave the other silently broken, and the two are set by the same line.
    #[test]
    fn a_photo_with_a_sidecar_keeps_exactly_what_it_stored() {
        let s = Settings::default();
        let stored = edit_model::Edits {
            // The opposite of what each preference would impose, so neither can pass by accident.
            rendering: Some(edit_model::Rendering::None),
            lens_correction: Some(edit_model::LensCorrection::default()),
            exposure: 0.75,
            ..edit_model::Edits::default()
        };
        let got = s.initial_edits(Some(stored.clone()));
        assert_eq!(
            got.rendering,
            Some(edit_model::Rendering::None),
            "the stored rendering was overwritten by the preference"
        );
        assert!(
            !got.applied_lens_correction().is_active(),
            "the stored lens correction was overwritten by the preference"
        );
        assert_eq!(got, stored, "a stored photo must come back untouched");
    }

    /// ⚠ Turning a preference off must reach a fresh photo too, or the setting is inert.
    #[test]
    fn the_preferences_off_open_a_fresh_photo_neutral() {
        let off = Settings {
            default_rendering: edit_model::Rendering::None,
            auto_lens_correction: false,
            ..Settings::default()
        };
        let fresh = off.initial_edits(None);
        assert_eq!(fresh.rendering, None, "undecided, not decided off");
        assert_eq!(fresh.lens_correction, None);
        assert_eq!(
            fresh,
            edit_model::Edits::default(),
            "and nothing else moved"
        );
    }

    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts *every*
    /// setting. The container-level `#[serde(default)]` is what stops an older file doing that.
    #[test]
    fn a_settings_file_from_before_default_rendering_still_loads() {
        let f = TempFile::new("pre_rendering");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let s = SettingsStore::new(f.0.clone()).load();
        assert!(!s.confirm_preset_delete, "the stored field was lost");
        assert_eq!(
            s.default_rendering,
            edit_model::Rendering::Camera,
            "the new field must come from Default, not from the enum's own default"
        );
    }

    #[test]
    fn corrections_are_applied_only_to_a_photo_with_no_sidecar() {
        // ⚠ The whole point of the preference living here rather than in `Edits`: a photo you
        // have already edited keeps exactly what it had, so nothing in the catalog re-renders.
        let on = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        assert!(
            on.initial_edits(None).applied_lens_correction().is_active(),
            "fresh photo"
        );
        assert!(
            !on.initial_edits(Some(edit_model::Edits::default()))
                .applied_lens_correction()
                .is_active(),
            "a photo with a sidecar must be left alone"
        );
    }

    #[test]
    fn the_preference_off_leaves_even_a_fresh_photo_neutral() {
        let off = Settings {
            auto_lens_correction: false,
            ..Settings::default()
        };
        assert_eq!(
            off.initial_lens_correction(),
            None,
            "undecided, not decided off — or every photo would get a sidecar"
        );
    }

    #[test]
    fn the_applied_default_enables_all_three_corrections() {
        let s = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        let l = s.initial_lens_correction().expect("the preference is on");
        assert!(l.distortion && l.tca && l.vignetting, "{l:?}");
        assert_eq!(l.scale, 100.0, "and autoscale by default");
    }

    #[test]
    fn a_settings_file_from_before_the_library_view_options_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting. The container-level `#[serde(default)]` is what stops that; this pins
        // it for the two fields added here.
        let f = TempFile::new("libview_old");
        std::fs::write(
            &f.0,
            r#"{"confirm_preset_delete":false,"auto_lens_correction":false}"#,
        )
        .unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert!(!got.confirm_preset_delete, "the stored settings survived");
        assert!(!got.auto_lens_correction);
        assert_eq!(got.library_sort, catalog::SortOrder::NameAsc);
        assert_eq!(got.library_cell, DEFAULT_CELL);
    }

    #[test]
    fn an_unusable_cell_size_never_reaches_the_grid() {
        // ⚠ The settings file is hand-editable, and a zero cell divides by zero when the column
        // count is worked out.
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(0).clamped_cell(), CELL_RANGE.0 as f32);
        assert_eq!(with(9999).clamped_cell(), CELL_RANGE.1 as f32);
        assert_eq!(with(200).clamped_cell(), 200.0, "a sane value is untouched");
    }

    #[test]
    fn the_grid_size_buttons_stop_at_the_ends() {
        // ⚠ Both directions, because a step that saturated only upward would let the cell walk
        // below the range and take the column count to zero.
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(CELL_RANGE.1).stepped_cell(true), CELL_RANGE.1);
        assert_eq!(with(CELL_RANGE.0).stepped_cell(false), CELL_RANGE.0);
        assert_eq!(
            with(DEFAULT_CELL).stepped_cell(true),
            DEFAULT_CELL + CELL_STEP
        );
        assert_eq!(
            with(DEFAULT_CELL).stepped_cell(false),
            DEFAULT_CELL - CELL_STEP
        );
        assert_eq!(
            with(0).stepped_cell(true),
            CELL_RANGE.0 + CELL_STEP,
            "a nonsense stored size is corrected by the first press"
        );
    }

    #[test]
    fn an_idle_gesture_leaves_the_grid_size_alone() {
        // ⚠ `zoom_delta` is a *factor* that is exactly 1.0 when nothing is happening, not a
        // delta. Anything that moved the size at 1.0 would shrink the grid on every idle frame.
        let s = Settings::default();
        assert_eq!(s.zoomed_cell(1.0), DEFAULT_CELL);
    }

    #[test]
    fn a_gentle_pinch_still_moves_the_grid() {
        // ⚠⚠ The failure this guards is silence, not wrongness. A trackpad reports ~1.01 per
        // frame; `168 * 1.01 = 169.68`, which rounds to 170 — but at 108 a 1.001 factor rounds
        // straight back to 108, and the control would appear dead until pinched hard.
        let small = Settings {
            library_cell: CELL_RANGE.0,
            ..Settings::default()
        };
        assert_eq!(small.zoomed_cell(1.001), CELL_RANGE.0 + 1);
        let big = Settings {
            library_cell: CELL_RANGE.1,
            ..Settings::default()
        };
        assert_eq!(big.zoomed_cell(0.999), CELL_RANGE.1 - 1);
    }

    #[test]
    fn zooming_stops_at_the_ends_and_survives_nonsense() {
        let with = |v: u32| Settings {
            library_cell: v,
            ..Settings::default()
        };
        assert_eq!(with(CELL_RANGE.1).zoomed_cell(4.0), CELL_RANGE.1);
        assert_eq!(with(CELL_RANGE.0).zoomed_cell(0.25), CELL_RANGE.0);
        // ⚠ `as u32` saturates rather than erroring, so a non-finite factor would otherwise
        // produce a silently absurd size.
        assert_eq!(with(200).zoomed_cell(f32::NAN), 200);
        assert_eq!(with(200).zoomed_cell(f32::INFINITY), 200);
        assert_eq!(with(200).zoomed_cell(0.0), 200);
        assert_eq!(with(200).zoomed_cell(-2.0), 200);
    }

    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_the_panel_sections_still_loads`.
    #[test]
    fn a_settings_file_from_before_the_snapshot_strip_still_loads() {
        let f = TempFile::new("presnapstrip");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert_eq!(
            got.snapshot_strip_height, DEFAULT_STRIP_HEIGHT,
            "and an absent one must give the default, not `u32`'s zero"
        );
    }

    #[test]
    fn a_nonsense_stored_strip_height_is_clamped_into_range() {
        let too_small = Settings {
            snapshot_strip_height: 0,
            ..Settings::default()
        };
        assert_eq!(
            too_small.clamped_strip_height(),
            STRIP_HEIGHT_RANGE.0 as f32
        );
        let too_big = Settings {
            snapshot_strip_height: 99_999,
            ..Settings::default()
        };
        assert_eq!(too_big.clamped_strip_height(), STRIP_HEIGHT_RANGE.1 as f32);
    }

    /// ⚠ The default must sit inside the range, or the very first frame is clamped to something
    /// nobody chose.
    #[test]
    fn the_default_strip_height_is_within_its_own_range() {
        assert!(DEFAULT_STRIP_HEIGHT >= STRIP_HEIGHT_RANGE.0);
        assert!(DEFAULT_STRIP_HEIGHT <= STRIP_HEIGHT_RANGE.1);
    }

    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_the_snapshot_strip_still_loads`.
    #[test]
    fn a_settings_file_from_before_dialog_sizes_still_loads() {
        let f = TempFile::new("predialogsizes");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert!(
            got.dialog_sizes.is_empty(),
            "and an absent map must read as empty, not fail the whole parse"
        );
    }

    /// ⚠ The file is user-editable and a zero width would make a dialog unreachable, so a stored
    /// size is clamped rather than trusted — `clamped_cell`'s rule.
    #[test]
    fn a_nonsense_stored_dialog_size_is_clamped_into_the_classs_range() {
        let mut s = Settings::default();
        s.dialog_sizes.insert("x".into(), [0, 0]);
        assert_eq!(
            s.dialog_size("x", (400.0, 120.0), (480.0, 320.0)),
            (400.0, 120.0),
            "clamped up to the class minimum"
        );

        s.dialog_sizes.insert("x".into(), [99_999, 99_999]);
        let (w, h) = s.dialog_size("x", (400.0, 120.0), (480.0, 320.0));
        assert!(w <= MAX_DIALOG_EDGE && h <= MAX_DIALOG_EDGE, "clamped down");
    }

    /// An unresized dialog gets its class default, not zero.
    #[test]
    fn an_unresized_dialog_gets_its_class_default() {
        let s = Settings::default();
        assert_eq!(
            s.dialog_size("never", (400.0, 120.0), (480.0, 320.0)),
            (480.0, 320.0)
        );
    }

    /// A remembered size is returned as stored.
    #[test]
    fn a_remembered_dialog_size_comes_back() {
        let mut s = Settings::default();
        s.remember_dialog_size("x", (612.0, 400.0));
        assert_eq!(
            s.dialog_size("x", (400.0, 120.0), (480.0, 320.0)),
            (612.0, 400.0)
        );
    }

    /// ⚠ `settings.json` is hand-editable, so the stored order can be any list at all. A section
    /// missing from it must still be reachable.
    #[test]
    fn a_stored_order_missing_a_section_gets_it_appended() {
        let s = Settings {
            panel_order: vec![PanelSection::Dates, PanelSection::Folders],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got[0], PanelSection::Dates, "the stored prefix is kept");
        assert_eq!(got[1], PanelSection::Folders);
        assert_eq!(
            got.len(),
            PanelSection::ALL.len(),
            "and every section is present"
        );
        for sec in PanelSection::ALL {
            assert!(got.contains(&sec), "{sec:?} must be reachable");
        }
    }

    #[test]
    fn a_duplicated_section_appears_once() {
        let s = Settings {
            panel_order: vec![
                PanelSection::Folders,
                PanelSection::Folders,
                PanelSection::Dates,
            ],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got.len(), PanelSection::ALL.len());
        assert_eq!(
            got.iter().filter(|x| **x == PanelSection::Folders).count(),
            1
        );
    }

    #[test]
    fn an_empty_stored_order_gives_the_default() {
        let s = Settings {
            panel_order: Vec::new(),
            ..Settings::default()
        };
        assert_eq!(s.panel_order(), PanelSection::ALL.to_vec());
    }

    /// ⚠ The default really is the order the panel shipped with — Folders first, because "where
    /// photos come from" is the outer concept.
    #[test]
    fn the_default_order_is_the_shipped_one() {
        assert_eq!(
            Settings::default().panel_order(),
            PanelSection::ALL.to_vec()
        );
        assert_eq!(PanelSection::ALL[0], PanelSection::Folders);
    }

    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_dialog_sizes_still_loads`.
    #[test]
    fn a_settings_file_from_before_the_panel_order_still_loads() {
        let f = TempFile::new("prepanelorder");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert_eq!(
            got.panel_order(),
            PanelSection::ALL.to_vec(),
            "and an absent order must give the default"
        );
    }

    #[test]
    fn a_settings_file_from_before_auto_advance_still_loads() {
        // ⚠⚠ The forgiving-load property, and the trap `snapshot_strip_height` and `write_xmp`
        // both document: `load()` is infallible, so a field that fails to parse does not surface
        // an error — it silently reverts *every* setting. A **field-level** `#[serde(default)]`
        // would be worse still: it would use `bool`'s own default of `false` and ship the feature
        // disabled for everyone who already has a settings file, which is most people.
        //
        // ⚠ Asserts the **value**, not the mechanism, so it catches either way round.
        let f = TempFile::new("auto_advance_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert!(
            loaded.auto_advance,
            "an existing settings file must get the feature, not silently opt out of it"
        );
    }
}
