//! Named export settings, saved one JSON file per preset.
//!
//! ⚠ The shape mirrors `presets::PresetStore`, but **not for the same reason**. That store
//! chose one-file-per-preset primarily so a develop preset is a *shareable artifact*; an
//! export preset contains an absolute local path and is not shareable, so that argument does
//! not carry. What does: no read-modify-write race on save or delete, one corrupt file
//! cannot take the rest down, and it is one pattern to understand rather than two.
//!
//! ⚠ **No trash, no undo, no delete confirmation** — deliberately unlike develop presets,
//! which have a 20-entry trash and Ctrl+Z restore. A develop preset can encode an hour of
//! grading; an export preset is three fields retyped in ten seconds. The asymmetry is
//! intentional, not an omission — please do not "fix" it.

use crate::batch_export::ExportOptions;
use crate::safe_name::sanitize;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One named set of export settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPreset {
    /// Display name. Stored inside the file as well as in the filename, because the filename
    /// is sanitized and the display name should survive that.
    pub name: String,
    pub options: ExportOptions,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportPresetError {
    #[error("an export preset needs a name")]
    EmptyName,
    #[error("export preset io: {0}")]
    Io(#[from] std::io::Error),
    #[error("export preset json: {0}")]
    Json(#[from] serde_json::Error),
}

/// A directory of export presets.
pub struct ExportPresetStore {
    dir: PathBuf,
}

impl ExportPresetStore {
    /// Rooted at `dir`. Taken by injection so tests never touch the real config directory —
    /// the pattern `DecodeWorker`, `PresetStore` and `SettingsStore` all use.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<config>/rustroom/export-presets/`.
    ///
    /// ⚠ Platform-specific via `dirs`. On **macOS** that is
    /// `~/Library/Application Support/rustroom/`, not `~/.config`.
    pub fn user_default() -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom");
        Self::new(base.join("export-presets"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every readable preset, sorted by name.
    ///
    /// Unreadable, malformed and non-`.json` entries are skipped rather than fatal: a
    /// hand-edited preset with a typo must not make the dropdown disappear. A missing
    /// directory is simply empty — the state on a fresh install.
    pub fn list(&self) -> Vec<ExportPreset> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<ExportPreset> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .filter_map(|p| std::fs::read_to_string(&p).ok())
            .filter_map(|t| serde_json::from_str::<ExportPreset>(&t).ok())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Write `preset`, replacing any existing one with the same name.
    pub fn save(&self, preset: &ExportPreset) -> Result<(), ExportPresetError> {
        let file = sanitize(&preset.name);
        if file.is_empty() {
            return Err(ExportPresetError::EmptyName);
        }
        std::fs::create_dir_all(&self.dir)?;
        let text = serde_json::to_string_pretty(preset)?;
        std::fs::write(self.dir.join(format!("{file}.json")), text)?;
        Ok(())
    }

    /// Delete the preset with this display name. Deleting one that is already gone is fine —
    /// the dropdown and the disk can disagree if a file was removed by hand.
    pub fn delete(&self, name: &str) -> Result<(), ExportPresetError> {
        let file = sanitize(name);
        if file.is_empty() {
            return Err(ExportPresetError::EmptyName);
        }
        match std::fs::remove_file(self.dir.join(format!("{file}.json"))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ExportPresetError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ A distinct prefix from `presets`' helper, and a distinct tag per test. Test binaries
    /// run in parallel and this codebase has already produced flaky tests from two of them
    /// racing on one temp path.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_export_presets_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn store(&self) -> ExportPresetStore {
            ExportPresetStore::new(self.0.join("export-presets"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn preset(name: &str, quality: u8) -> ExportPreset {
        ExportPreset {
            name: name.to_string(),
            options: ExportOptions {
                dest_dir: PathBuf::from("/tmp/rustroom_web"),
                quality,
                max_edge: Some(2048),
                ..ExportOptions::default()
            },
        }
    }

    #[test]
    fn a_preset_carries_the_export_format() {
        let d = TempDir::new("format");
        let store = d.store();
        store
            .save(&ExportPreset {
                name: "Archive".into(),
                options: ExportOptions {
                    format: export::ExportFormat::Tiff16,
                    ..ExportOptions::default()
                },
            })
            .unwrap();
        assert_eq!(store.list()[0].options.format, export::ExportFormat::Tiff16);
    }

    #[test]
    fn a_preset_carries_the_demosaic_choice() {
        let d = TempDir::new("demosaic");
        let store = d.store();
        let p = ExportPreset {
            name: "Archive".into(),
            options: ExportOptions {
                demosaic: raw_decode::Demosaic::Best,
                ..ExportOptions::default()
            },
        };
        store.save(&p).unwrap();
        assert_eq!(store.list()[0].options.demosaic, raw_decode::Demosaic::Best);
    }

    #[test]
    fn a_preset_round_trips_through_the_store() {
        let d = TempDir::new("round_trip");
        let store = d.store();
        let p = preset("Web 2048", 82);
        store.save(&p).unwrap();
        assert_eq!(store.list(), vec![p]);
    }

    #[test]
    fn a_missing_directory_lists_empty_rather_than_erroring() {
        // The state on a fresh install; the dropdown must simply be empty.
        let d = TempDir::new("missing");
        assert!(d.store().list().is_empty());
    }

    #[test]
    fn saving_the_same_name_replaces_rather_than_duplicates() {
        let d = TempDir::new("replace");
        let store = d.store();
        store.save(&preset("Web", 60)).unwrap();
        store.save(&preset("Web", 95)).unwrap();
        let all = store.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].options.quality, 95);
    }

    #[test]
    fn presets_come_back_sorted_by_name() {
        let d = TempDir::new("sorted");
        let store = d.store();
        for n in ["Zip", "Archive", "Mid"] {
            store.save(&preset(n, 80)).unwrap();
        }
        let names: Vec<String> = store.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Archive", "Mid", "Zip"]);
    }

    #[test]
    fn deleting_is_immediate_and_deleting_twice_is_fine() {
        // No trash, unlike develop presets — see the module doc for why.
        let d = TempDir::new("delete");
        let store = d.store();
        store.save(&preset("Gone", 80)).unwrap();
        store.delete("Gone").unwrap();
        assert!(store.list().is_empty());
        store.delete("Gone").unwrap();
        store.delete("never existed").unwrap();
    }

    #[test]
    fn a_name_with_nothing_usable_is_rejected() {
        let d = TempDir::new("empty_name");
        let store = d.store();
        assert!(matches!(
            store.save(&preset("   ", 80)),
            Err(ExportPresetError::EmptyName)
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_traversal_name_stays_inside_the_directory() {
        // ⚠ The shared `safe_name` guard, exercised through this store as well — the point
        // of sharing it is that both callers get it, so both callers should prove it.
        let d = TempDir::new("traversal");
        let store = d.store();
        store.save(&preset("../../evil", 80)).unwrap();
        assert_eq!(store.list().len(), 1, "the preset should be readable back");
        let escaped = d.0.parent().unwrap().join("evil.json");
        assert!(!escaped.exists(), "a file escaped to {}", escaped.display());
    }

    #[test]
    fn a_malformed_file_is_skipped_rather_than_fatal() {
        let d = TempDir::new("malformed");
        let store = d.store();
        store.save(&preset("Good", 80)).unwrap();
        std::fs::write(store.dir().join("broken.json"), "{ not json").unwrap();
        assert_eq!(store.list().len(), 1, "the good preset must still list");
    }
}
