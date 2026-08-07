//! Named `Edits` saved to disk.
//!
//! One JSON file per preset, not one file holding all of them: that makes a preset a
//! shareable artifact, makes add and delete plain file operations with no read-modify-write
//! race, and means one corrupt preset cannot take the rest down with it.

use edit_model::{EditGroups, Edits};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A named set of edits plus the groups it carries.
///
/// The preset stores its **own** group mask, chosen when it is created, so a "punchy tone"
/// preset never disturbs the photo's white balance. Copy/paste picks groups at paste time
/// instead; the shared `EditGroups` type is what makes the two one feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Display name. Stored inside the file as well as in the filename, because the
    /// filename is sanitized and the display name should survive that.
    pub name: String,
    pub groups: EditGroups,
    pub edits: Edits,
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("a preset needs a name")]
    EmptyName,
    #[error("preset io: {0}")]
    Io(#[from] std::io::Error),
    #[error("preset json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Filename-safe form of a preset name: anything outside `[A-Za-z0-9 _-]` becomes `_`.
///
/// ⚠ Load-bearing, not cosmetic. The name is free text from the UI and becomes a path, so
/// `../../evil` must not escape the presets directory.
///
/// This is a many-to-one map, so `a/b` and `a_b` do share a file and the later save wins —
/// see `distinct_names_can_still_collide_on_one_file`. Accepted: hashing the name would fix
/// it at the cost of a directory nobody can read by hand, which is most of the point of
/// one file per preset. Only a name that sanitizes to *empty* is rejected outright.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Presets on disk, in one flat directory.
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    /// `dir` is injected so tests run against a temp directory and never touch the real
    /// home directory — the same pattern `DecodeWorker` uses for its decode fn.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The user's presets directory: `~/.config/rustroom/presets` (platform equivalent
    /// elsewhere). Falls back to a relative path if the OS reports no config dir, which
    /// keeps this infallible.
    pub fn user_default() -> Self {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom")
            .join("presets");
        Self::new(dir)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every readable preset, sorted by name.
    ///
    /// Unreadable, malformed and non-`.json` entries are **skipped rather than fatal**: a
    /// hand-edited preset with a typo must not make the preset panel disappear. A missing
    /// directory is simply empty — that is the state on a fresh install.
    pub fn list(&self) -> Vec<Preset> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Preset> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .filter_map(|p| std::fs::read_to_string(&p).ok())
            .filter_map(|text| serde_json::from_str::<Preset>(&text).ok())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Write `preset`, replacing any existing preset with the same name.
    pub fn save(&self, preset: &Preset) -> Result<(), PresetError> {
        let file = sanitize(&preset.name);
        if file.is_empty() {
            return Err(PresetError::EmptyName);
        }
        std::fs::create_dir_all(&self.dir)?;
        let text = serde_json::to_string_pretty(preset)?;
        std::fs::write(self.dir.join(format!("{file}.json")), text)?;
        Ok(())
    }

    /// Remove the preset with this display name. Deleting one that is already gone is fine.
    pub fn delete(&self, name: &str) -> Result<(), PresetError> {
        let file = sanitize(name);
        if file.is_empty() {
            return Err(PresetError::EmptyName);
        }
        let path = self.dir.join(format!("{file}.json"));
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PresetError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::Edits;

    /// A unique temp directory for one test, removed on drop. Keeps tests off the real
    /// home directory without adding a `tempfile` dependency.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_presets_{tag}"));
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

    fn preset(name: &str, exposure: f32) -> Preset {
        Preset {
            name: name.to_string(),
            groups: EditGroups::default(),
            edits: Edits {
                exposure,
                ..Edits::default()
            },
        }
    }

    #[test]
    fn save_list_delete_round_trip() {
        let dir = TempDir::new("roundtrip");
        let store = PresetStore::new(dir.0.clone());
        assert!(store.list().is_empty());

        store.save(&preset("Punchy", 1.0)).unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Punchy");
        assert_eq!(got[0].edits.exposure, 1.0);
        assert_eq!(got[0].groups, EditGroups::default());

        store.delete("Punchy").unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn list_is_sorted_by_name() {
        let dir = TempDir::new("sorted");
        let store = PresetStore::new(dir.0.clone());
        for n in ["Zebra", "Alpha", "Mango"] {
            store.save(&preset(n, 0.0)).unwrap();
        }
        let names: Vec<String> = store.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Alpha", "Mango", "Zebra"]);
    }

    #[test]
    fn saving_an_existing_name_overwrites_it() {
        let dir = TempDir::new("overwrite");
        let store = PresetStore::new(dir.0.clone());
        store.save(&preset("Look", 1.0)).unwrap();
        store.save(&preset("Look", 2.0)).unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1, "a second save must not create a second file");
        assert_eq!(got[0].edits.exposure, 2.0);
    }

    #[test]
    fn a_malformed_preset_is_skipped_not_fatal() {
        // A hand-edited preset with a typo must not make the whole panel disappear.
        let dir = TempDir::new("malformed");
        let store = PresetStore::new(dir.0.clone());
        store.save(&preset("Good", 1.0)).unwrap();
        std::fs::write(dir.0.join("broken.json"), "{ not json").unwrap();
        std::fs::write(dir.0.join("notes.txt"), "ignore me").unwrap();

        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Good");
    }

    #[test]
    fn the_display_name_survives_sanitizing() {
        // The filename is sanitized; the name inside the file is not.
        let dir = TempDir::new("display");
        let store = PresetStore::new(dir.0.clone());
        store.save(&preset("Warm / Punchy!", 1.0)).unwrap();
        assert_eq!(store.list()[0].name, "Warm / Punchy!");
    }

    #[test]
    fn a_traversing_name_stays_inside_the_directory() {
        // ⚠ The name is free text from the UI and becomes a path. This is the one place in
        // this feature where user input reaches the filesystem.
        let dir = TempDir::new("traversal");
        let outside = dir.0.join("outside_marker");
        std::fs::create_dir_all(&outside).unwrap();
        let store = PresetStore::new(outside.clone());

        store.save(&preset("../../evil", 1.0)).unwrap();
        // Everything written must live directly in `outside`, not above it.
        let escaped = dir.0.join("evil.json");
        assert!(!escaped.exists(), "preset escaped its directory");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].name, "../../evil");
    }

    #[test]
    fn a_name_that_sanitizes_to_nothing_is_rejected() {
        let dir = TempDir::new("empty");
        let store = PresetStore::new(dir.0.clone());
        for name in ["", "   "] {
            assert!(
                matches!(store.save(&preset(name, 1.0)), Err(PresetError::EmptyName)),
                "{name:?} should be rejected"
            );
        }
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_name_of_only_punctuation_is_saved_under_a_safe_filename() {
        // Not rejected — it sanitizes to "___", which is a perfectly good filename. Only a
        // name that sanitizes to *empty* has nowhere to go.
        let dir = TempDir::new("punct");
        let store = PresetStore::new(dir.0.clone());
        store.save(&preset("///", 1.0)).unwrap();
        assert!(dir.0.join("___.json").exists());
        assert_eq!(store.list()[0].name, "///");
    }

    #[test]
    fn sanitize_keeps_readable_names_readable() {
        assert_eq!(sanitize("Warm Punchy 2"), "Warm Punchy 2");
        assert_eq!(sanitize("high-key_v2"), "high-key_v2");
        assert_eq!(sanitize("../../evil"), "______evil");
        assert_eq!(sanitize("a/b\\c"), "a_b_c");
    }

    #[test]
    fn distinct_names_can_still_collide_on_one_file() {
        // Documenting a known limitation rather than pretending it away: sanitizing is a
        // many-to-one map, so "a/b" and "a_b" share a file and the second wins. Acceptable
        // because the alternative (hashing) makes the directory unreadable by hand, which
        // is most of the point of one-file-per-preset.
        let dir = TempDir::new("collide");
        let store = PresetStore::new(dir.0.clone());
        store.save(&preset("a/b", 1.0)).unwrap();
        store.save(&preset("a_b", 2.0)).unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a_b");
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        // Nothing has been saved yet on a fresh install, so the directory does not exist.
        let store = PresetStore::new(std::env::temp_dir().join("rustroom_presets_absent_dir"));
        assert!(store.list().is_empty());
    }
}
