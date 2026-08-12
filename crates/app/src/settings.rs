//! Persisted application preferences.
//!
//! Deliberately tiny. Other settings (`show_filmstrip`, panel widths, last folder, export
//! defaults) move in when they are actually wanted, not speculatively — egui panel sizes in
//! particular are fiddly to save and restore, and that belongs in whatever spec needs them.

use crate::batch_export::ExportOptions;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_preset_delete: true,
            export: ExportOptions::default(),
            confirm_export_preset_delete: true,
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
}
