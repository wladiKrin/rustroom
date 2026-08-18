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
    /// Which theme to render in. Dark by default: a light surround measurably changes how you
    /// judge exposure, so a photo editor defaults to dark and offers the rest.
    pub theme: ThemeChoice,
    /// The accent colour for active state.
    pub accent: Accent,
    /// Which drag gesture pans the canvas. Scroll and two-finger swipe always pan.
    pub pan_drag: PanDrag,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_preset_delete: true,
            export: ExportOptions::default(),
            confirm_export_preset_delete: true,
            view_demosaic: Demosaic::default(),
            theme: ThemeChoice::default(),
            accent: Accent::default(),
            pan_drag: PanDrag::default(),
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
            theme: ThemeChoice::System,
            accent: Accent::Teal,
            // A non-default value on purpose: it proves the field is actually persisted
            // rather than being re-defaulted on load.
            pan_drag: PanDrag::Secondary,
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
}
