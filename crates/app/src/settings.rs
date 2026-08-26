//! Persisted application preferences.
//!
//! Deliberately tiny. Other settings (`show_filmstrip`, panel widths, last folder, export
//! defaults) move in when they are actually wanted, not speculatively — egui panel sizes in
//! particular are fiddly to save and restore, and that belongs in whatever spec needs them.

use crate::batch_export::ExportOptions;
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

/// User preferences that survive a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
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
    /// Defaults to `Fast`, which is what `DecodeOptions::linear_scene()` already hardcodes, so
    /// nobody who ignores this setting sees any change.
    pub view_demosaic: Demosaic,
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
}

/// The Library grid's default cell size, and the size its type scale is relative to.
pub const DEFAULT_CELL: u32 = 168;
/// The smallest and largest cell the buttons will reach.
pub const CELL_RANGE: (u32, u32) = (108, 320);
/// How much one press of the grid-size buttons moves the cell.
pub const CELL_STEP: u32 = 28;

impl Settings {
    /// The grid cell size in points, guaranteed usable.
    pub fn clamped_cell(&self) -> f32 {
        self.library_cell.clamp(CELL_RANGE.0, CELL_RANGE.1) as f32
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
            confirm_preset_delete: true,
            export: ExportOptions::default(),
            confirm_export_preset_delete: true,
            view_demosaic: Demosaic::default(),
            camera_profiles: std::collections::BTreeMap::new(),
            display_space: color::OutputSpace::default(),
            theme: ThemeChoice::default(),
            accent: Accent::default(),
            pan_drag: PanDrag::default(),
            auto_lens_correction: true,
            library_sort: catalog::SortOrder::default(),
            library_cell: DEFAULT_CELL,
        }
    }
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
    /// What a newly-opened photo's lens corrections should start as.
    ///
    /// Extracted as a pure function so the rule is testable without a window — the decision is
    /// the part worth testing, and the egui glue is not.
    pub fn initial_lens_correction(&self, has_sidecar: bool) -> edit_model::LensCorrection {
        if self.auto_lens_correction && !has_sidecar {
            edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            }
        } else {
            edit_model::LensCorrection::default()
        }
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

    #[test]
    fn a_settings_file_from_before_the_export_format_still_loads() {
        // The forgiving-load property `ExportOptions::default()` exists for.
        let f = TempFile::new("format_old");
        std::fs::write(&f.0, r#"{"export":{"quality":80}}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert_eq!(loaded.export.quality, 80, "the old field must survive");
        assert_eq!(loaded.export.format, export::ExportFormat::default());
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
            confirm_preset_delete: false,
            confirm_export_preset_delete: false,
            // Listed explicitly rather than via `..Default::default()`: this literal being
            // exhaustive is what makes a new `Settings` field a compile error here, forcing
            // a decision about whether the round trip should carry it.
            view_demosaic: Demosaic::Better,
            display_space: color::OutputSpace::DisplayP3,
            camera_profiles: std::collections::BTreeMap::new(),
            theme: ThemeChoice::System,
            accent: Accent::Teal,
            // A non-default value on purpose: it proves the field is actually persisted
            // rather than being re-defaulted on load.
            pan_drag: PanDrag::Secondary,
            // Both non-default, for the same reason: a field re-defaulted on load would look
            // like it round-tripped.
            library_sort: catalog::SortOrder::ModifiedDesc,
            library_cell: DEFAULT_CELL + CELL_STEP,
            // Likewise non-default: the preference defaults to *on*, so persisting `false` is
            // what proves it round-trips rather than being re-defaulted.
            auto_lens_correction: false,
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
            raw_decode::Demosaic::Fast,
            "and the new one must default to what linear_scene() already hardcoded"
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

    #[test]
    fn corrections_are_applied_only_to_a_photo_with_no_sidecar() {
        // ⚠ The whole point of the preference living here rather than in `Edits`: a photo you
        // have already edited keeps exactly what it had, so nothing in the catalog re-renders.
        let on = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        assert!(on.initial_lens_correction(false).is_active(), "fresh photo");
        assert!(
            !on.initial_lens_correction(true).is_active(),
            "a photo with a sidecar must be left alone"
        );
    }

    #[test]
    fn the_preference_off_leaves_even_a_fresh_photo_neutral() {
        let off = Settings {
            auto_lens_correction: false,
            ..Settings::default()
        };
        assert!(!off.initial_lens_correction(false).is_active());
    }

    #[test]
    fn the_applied_default_enables_all_three_corrections() {
        let s = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        let l = s.initial_lens_correction(false);
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
}
