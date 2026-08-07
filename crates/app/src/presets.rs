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

/// How many deleted presets stay recoverable.
///
/// A cap on *entries*, not bytes, and deliberately so: a worst-case preset measures ~2.8 KB,
/// so 20 of them is ~56 KB. Disk space is never the binding constraint here — what degrades
/// is the directory's comprehensibility. An age-based cap was rejected for a worse reason:
/// the entry you want back is precisely the one that just silently expired.
pub const TRASH_CAP: usize = 20;

/// Presets on disk, plus a bounded trash of deleted ones.
pub struct PresetStore {
    dir: PathBuf,
    trash_dir: PathBuf,
}

impl PresetStore {
    /// Both directories are injected so tests run against a temp directory and never touch
    /// the real home directory — the same pattern `DecodeWorker` uses for its decode fn.
    ///
    /// `trash_dir` is a **sibling** of `dir`, never a subdirectory of it: nesting would
    /// oblige every present and future globber of the presets directory to remember to
    /// exclude it, which is a standing trap. This way [`Self::list`] needs no filter.
    pub fn new(dir: PathBuf, trash_dir: PathBuf) -> Self {
        Self { dir, trash_dir }
    }

    /// The user's presets and trash directories, under `<platform config dir>/rustroom/`.
    ///
    /// ⚠ That is **not** `~/.config` everywhere — `dirs::config_dir()` follows each
    /// platform's own convention: `~/Library/Application Support` on macOS,
    /// `$XDG_CONFIG_HOME` (usually `~/.config`) on Linux, `%APPDATA%` on Windows. Use
    /// `PresetStore::dir()` when you need to tell a user where their presets actually live.
    ///
    /// Falls back to a relative path if the OS reports no config dir, which keeps this
    /// infallible.
    pub fn user_default() -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom");
        Self::new(base.join("presets"), base.join("trash"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn trash_dir(&self) -> &Path {
        &self.trash_dir
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

    /// Move the preset with this display name to the trash, then evict anything beyond
    /// [`TRASH_CAP`]. Deleting one that is already gone is fine.
    pub fn delete(&self, name: &str) -> Result<(), PresetError> {
        let file = sanitize(name);
        if file.is_empty() {
            return Err(PresetError::EmptyName);
        }
        let path = self.dir.join(format!("{file}.json"));
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(PresetError::Io(e)),
        };
        std::fs::create_dir_all(&self.trash_dir)?;
        std::fs::write(self.trash_path(&file), text)?;
        std::fs::remove_file(&path)?;
        self.evict_trash();
        Ok(())
    }

    /// A free trash path for `file`, prefixed with the deletion time.
    ///
    /// The timestamp is doing two jobs. It gives a total order for eviction and undo without
    /// trusting file mtime — rsync and backup restores rewrite mtime, which is not a fact
    /// about when anyone deleted anything — and it keeps a re-created, re-deleted preset from
    /// clobbering its own earlier trashed copy. Zero-padded to 13 digits so lexicographic
    /// order is chronological order.
    ///
    /// ⚠ The `+= 1` loop is not paranoia: two deletions inside one millisecond are trivially
    /// easy to produce (any test doing two in a row does it), and without the bump the second
    /// would silently overwrite the first.
    fn trash_path(&self, file: &str) -> PathBuf {
        let mut ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        loop {
            let p = self.trash_dir.join(format!("{ms:013}-{file}.json"));
            if !p.exists() {
                return p;
            }
            ms += 1;
        }
    }

    /// Trash entries oldest-first. Sorting the names works because of the padded prefix.
    fn trash_files(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.trash_dir) else {
            return Vec::new();
        };
        let mut v: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        v.sort();
        v
    }

    /// Drop the oldest entries beyond the cap. Best-effort: a file we cannot remove is
    /// not worth failing a deletion over.
    fn evict_trash(&self) {
        let files = self.trash_files();
        if let Some(excess) = files.len().checked_sub(TRASH_CAP) {
            for p in &files[..excess] {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// The newest readable trash entry, with its path. Malformed entries are skipped, the
    /// same way [`Self::list`] skips them.
    fn latest_trashed(&self) -> Option<(PathBuf, Preset)> {
        self.trash_files().into_iter().rev().find_map(|path| {
            let preset = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Preset>(&t).ok())?;
            Some((path, preset))
        })
    }

    /// The name of the newest trashed preset, for the undo affordance's label.
    pub fn latest_trashed_name(&self) -> Option<String> {
        self.latest_trashed().map(|(_, p)| p.name)
    }

    /// Restore the newest trashed preset. `Ok(None)` when the trash is empty.
    ///
    /// Restoring over a live preset of the same name **overwrites** it, consistent with
    /// [`Self::save`]. Returns what was restored, for the status line.
    pub fn undo_delete(&self) -> Result<Option<Preset>, PresetError> {
        let Some((path, preset)) = self.latest_trashed() else {
            return Ok(None);
        };
        self.save(&preset)?;
        let _ = std::fs::remove_file(&path);
        Ok(Some(preset))
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

        /// A store rooted here, with the sibling layout `user_default` uses.
        fn store(&self) -> PresetStore {
            PresetStore::new(self.0.join("presets"), self.0.join("trash"))
        }

        fn trash_count(&self) -> usize {
            std::fs::read_dir(self.0.join("trash"))
                .map(|d| d.flatten().count())
                .unwrap_or(0)
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
        let store = dir.store();
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
        let store = dir.store();
        for n in ["Zebra", "Alpha", "Mango"] {
            store.save(&preset(n, 0.0)).unwrap();
        }
        let names: Vec<String> = store.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Alpha", "Mango", "Zebra"]);
    }

    #[test]
    fn saving_an_existing_name_overwrites_it() {
        let dir = TempDir::new("overwrite");
        let store = dir.store();
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
        let store = dir.store();
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
        let store = dir.store();
        store.save(&preset("Warm / Punchy!", 1.0)).unwrap();
        assert_eq!(store.list()[0].name, "Warm / Punchy!");
    }

    #[test]
    fn a_traversing_name_stays_inside_the_directory() {
        // ⚠ The name is free text from the UI and becomes a path. This is the one place in
        // this feature where user input reaches the filesystem.
        let dir = TempDir::new("traversal");
        let store = dir.store();

        store.save(&preset("../../evil", 1.0)).unwrap();
        // Everything written must live inside `presets/`, not above it.
        let escaped = dir.0.join("evil.json");
        assert!(!escaped.exists(), "preset escaped its directory");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].name, "../../evil");
    }

    #[test]
    fn a_name_that_sanitizes_to_nothing_is_rejected() {
        let dir = TempDir::new("empty");
        let store = dir.store();
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
        let store = dir.store();
        store.save(&preset("///", 1.0)).unwrap();
        assert!(dir.0.join("presets").join("___.json").exists());
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
        let store = dir.store();
        store.save(&preset("a/b", 1.0)).unwrap();
        store.save(&preset("a_b", 2.0)).unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a_b");
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        // Nothing has been saved yet on a fresh install, so the directory does not exist.
        let base = std::env::temp_dir().join("rustroom_presets_absent_dir");
        let store = PresetStore::new(base.join("presets"), base.join("trash"));
        assert!(store.list().is_empty());
        assert_eq!(store.latest_trashed_name(), None);
    }

    #[test]
    fn deleting_moves_the_preset_to_the_trash() {
        let dir = TempDir::new("to_trash");
        let store = dir.store();
        store.save(&preset("Punchy", 1.0)).unwrap();
        store.delete("Punchy").unwrap();

        assert!(store.list().is_empty(), "still live after delete");
        assert_eq!(dir.trash_count(), 1);
        assert_eq!(store.latest_trashed_name().as_deref(), Some("Punchy"));
    }

    #[test]
    fn undo_delete_brings_it_back_and_empties_that_slot() {
        let dir = TempDir::new("undo");
        let store = dir.store();
        store.save(&preset("Punchy", 1.5)).unwrap();
        store.delete("Punchy").unwrap();

        let restored = store.undo_delete().unwrap().expect("something to restore");
        assert_eq!(restored.name, "Punchy");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].edits.exposure, 1.5);
        assert_eq!(dir.trash_count(), 0, "the entry should leave the trash");
        assert_eq!(store.latest_trashed_name(), None);
    }

    #[test]
    fn undo_delete_on_an_empty_trash_is_ok_none() {
        let dir = TempDir::new("undo_empty");
        assert!(dir.store().undo_delete().unwrap().is_none());
    }

    #[test]
    fn deleting_the_same_name_twice_keeps_both_copies() {
        // ⚠ Without the timestamp prefix the second deletion would clobber the first
        // trashed copy, and the older version would be gone with no sign of it.
        let dir = TempDir::new("same_name");
        let store = dir.store();
        store.save(&preset("Look", 1.0)).unwrap();
        store.delete("Look").unwrap();
        store.save(&preset("Look", 2.0)).unwrap();
        store.delete("Look").unwrap();

        assert_eq!(dir.trash_count(), 2);
        // Newest first: the second version comes back before the first.
        assert_eq!(store.undo_delete().unwrap().unwrap().edits.exposure, 2.0);
        assert_eq!(store.undo_delete().unwrap().unwrap().edits.exposure, 1.0);
    }

    #[test]
    fn deletions_within_one_millisecond_both_survive() {
        // ⚠ This loop runs far faster than the millisecond clock ticks, so without the
        // collision bump in `trash_path` most of these would overwrite each other.
        let dir = TempDir::new("same_ms");
        let store = dir.store();
        for i in 0..8 {
            store.save(&preset(&format!("P{i}"), i as f32)).unwrap();
            store.delete(&format!("P{i}")).unwrap();
        }
        assert_eq!(dir.trash_count(), 8, "trashed entries overwrote each other");
    }

    #[test]
    fn the_trash_never_exceeds_the_cap_and_drops_the_oldest() {
        let dir = TempDir::new("cap");
        let store = dir.store();
        let total = TRASH_CAP + 5;
        for i in 0..total {
            store.save(&preset(&format!("P{i:03}"), i as f32)).unwrap();
            store.delete(&format!("P{i:03}")).unwrap();
        }
        assert_eq!(dir.trash_count(), TRASH_CAP);
        // The newest survives...
        assert_eq!(
            store.latest_trashed_name().as_deref(),
            Some(format!("P{:03}", total - 1).as_str())
        );
        // ...and it is the oldest five that were evicted: restoring everything left should
        // never yield P000..P004.
        let mut names = Vec::new();
        while let Some(p) = store.undo_delete().unwrap() {
            names.push(p.name);
        }
        assert_eq!(names.len(), TRASH_CAP);
        for i in 0..5 {
            let gone = format!("P{i:03}");
            assert!(!names.contains(&gone), "{gone} should have been evicted");
        }
    }

    #[test]
    fn restoring_over_a_live_preset_overwrites_it() {
        // Documented behaviour, consistent with `save`: an undo should not leave two
        // near-identical presets behind.
        let dir = TempDir::new("restore_clash");
        let store = dir.store();
        store.save(&preset("Look", 1.0)).unwrap();
        store.delete("Look").unwrap();
        store.save(&preset("Look", 9.0)).unwrap();

        store.undo_delete().unwrap().unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].edits.exposure, 1.0, "the trashed version should win");
    }

    #[test]
    fn a_malformed_trash_entry_is_skipped_not_fatal() {
        let dir = TempDir::new("bad_trash");
        let store = dir.store();
        store.save(&preset("Good", 1.0)).unwrap();
        store.delete("Good").unwrap();
        // Sorts after the real entry, so it is the first one `latest_trashed` examines.
        std::fs::write(dir.0.join("trash").join("9999999999999-x.json"), "{ nope").unwrap();

        assert_eq!(store.latest_trashed_name().as_deref(), Some("Good"));
        assert_eq!(store.undo_delete().unwrap().unwrap().name, "Good");
    }

    #[test]
    fn deleting_something_that_is_already_gone_is_not_an_error() {
        let dir = TempDir::new("gone");
        let store = dir.store();
        store.delete("Nothing").unwrap();
        assert_eq!(dir.trash_count(), 0, "nothing should be trashed");
    }
}
