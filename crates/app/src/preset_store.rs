//! Develop presets on disk: one JSON file per preset, **one level of folders** as subdirectories
//! (M10.9a), and a bounded trash of deleted ones.
//!
//! One file per preset, not one file holding all of them: that makes a preset a shareable artifact,
//! makes add and delete plain file operations with no read-modify-write race, and means one corrupt
//! preset cannot take the rest down with it. ⚠ **The file format did not change with folders** — the
//! folder is where the file is, never inside it, so an exported preset is exactly the stored file.
//!
//! ⚠⚠ **Nothing is ever overwritten by a rename, a move or a folder change**: a name the target
//! already holds is refused ([`PresetError::Taken`]). `save` still replaces a preset of the same name
//! in the same folder — that is what saving under an existing name has always meant here.

use crate::presets::Preset;
use crate::safe_name::{folder_name, sanitize};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("a preset needs a name")]
    EmptyName,
    #[error("there is already a “{0}” there")]
    Taken(String),
    #[error("no preset “{0}”")]
    Missing(String),
    #[error("preset io: {0}")]
    Io(#[from] std::io::Error),
    #[error("preset json: {0}")]
    Json(#[from] serde_json::Error),
}

/// How many deleted presets stay recoverable.
///
/// A cap on *entries*, not bytes, and deliberately so: a worst-case preset measures ~2.8 KB,
/// so 20 of them is ~56 KB. Disk space is never the binding constraint here — what degrades
/// is the directory's comprehensibility. An age-based cap was rejected for a worse reason:
/// the entry you want back is precisely the one that just silently expired.
pub const TRASH_CAP: usize = 20;

/// A trash entry: the preset and the folder it was deleted from (M10.9a).
///
/// ⚠ An entry written before folders existed is a bare `Preset`, and still reads — as no folder.
#[derive(Serialize, Deserialize)]
struct TrashEntry {
    folder: Option<String>,
    preset: Preset,
}

fn read_trash_entry(text: &str) -> Option<(Option<String>, Preset)> {
    if let Ok(e) = serde_json::from_str::<TrashEntry>(text) {
        return Some((e.folder, e.preset));
    }
    serde_json::from_str::<Preset>(text).ok().map(|p| (None, p))
}

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
    /// exclude it — and since M10.9a every subdirectory of `dir` IS a folder of presets.
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

    /// The directory a folder's presets live in — the presets directory itself for no folder.
    pub fn folder_dir(&self, folder: Option<&str>) -> PathBuf {
        match folder {
            Some(f) => self.dir.join(f),
            None => self.dir.clone(),
        }
    }

    /// The file a preset of this name lives in, in this folder — `None` for a name that
    /// sanitizes to nothing.
    pub fn path_of(&self, folder: Option<&str>, name: &str) -> Option<PathBuf> {
        let file = sanitize(name);
        (!file.is_empty()).then(|| self.folder_dir(folder).join(format!("{file}.json")))
    }

    /// Whether a preset of this name is in this folder.
    pub fn exists(&self, folder: Option<&str>, name: &str) -> bool {
        self.path_of(folder, name).is_some_and(|p| p.exists())
    }

    /// Every readable preset — the ones with no folder first, then each folder's — sorted by name
    /// within each.
    ///
    /// Unreadable, malformed and non-`.json` entries are **skipped rather than fatal**: a
    /// hand-edited preset with a typo must not make the preset panel disappear. A missing
    /// directory is simply empty — that is the state on a fresh install.
    pub fn list(&self) -> Vec<Preset> {
        let mut out = read_dir_presets(&self.dir, None);
        for f in self.folders() {
            out.extend(read_dir_presets(&self.dir.join(&f), Some(&f)));
        }
        out
    }

    /// Every folder, sorted, the empty ones included. ⚠ A directory whose name starts with `.` is
    /// not a folder — the hidden files a platform leaves behind.
    pub fn folders(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut v: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        v
    }

    /// Write `preset` into its folder, replacing any preset with the same name there.
    pub fn save(&self, preset: &Preset) -> Result<(), PresetError> {
        let path = self
            .path_of(preset.folder.as_deref(), &preset.name)
            .ok_or(PresetError::EmptyName)?;
        std::fs::create_dir_all(self.folder_dir(preset.folder.as_deref()))?;
        std::fs::write(path, serde_json::to_string_pretty(preset)?)?;
        Ok(())
    }

    /// The preset of this name in this folder, read from its file.
    pub fn get(&self, folder: Option<&str>, name: &str) -> Option<Preset> {
        let path = self.path_of(folder, name)?;
        let text = std::fs::read_to_string(path).ok()?;
        let mut p = serde_json::from_str::<Preset>(&text).ok()?;
        p.folder = folder.map(str::to_string);
        Some(p)
    }

    /// Move the preset with this name in this folder to the trash, then evict anything beyond
    /// [`TRASH_CAP`]. Deleting one that is already gone is fine.
    pub fn delete(&self, folder: Option<&str>, name: &str) -> Result<(), PresetError> {
        let path = self.path_of(folder, name).ok_or(PresetError::EmptyName)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(PresetError::Io(e)),
        };
        let preset: Preset = serde_json::from_str(&text)?;
        let entry = TrashEntry {
            folder: folder.map(str::to_string),
            preset,
        };
        std::fs::create_dir_all(&self.trash_dir)?;
        std::fs::write(
            self.trash_path(&sanitize(name)),
            serde_json::to_string_pretty(&entry)?,
        )?;
        std::fs::remove_file(&path)?;
        self.evict_trash();
        Ok(())
    }

    /// Rename the preset `name` in `folder` to `new`, keeping its folder. Returns it renamed.
    ///
    /// ⚠ A name the folder already holds is refused, never overwritten — unless it is the same file
    /// (a change of case only, which a case-insensitive disk sees as one name).
    pub fn rename(
        &self,
        folder: Option<&str>,
        name: &str,
        new: &str,
    ) -> Result<Preset, PresetError> {
        let new = new.trim();
        let from = self.path_of(folder, name).ok_or(PresetError::EmptyName)?;
        let to = self.path_of(folder, new).ok_or(PresetError::EmptyName)?;
        let mut p = self
            .get(folder, name)
            .ok_or_else(|| PresetError::Missing(name.to_string()))?;
        let same_file = same_name(&from, &to);
        if !same_file && to.exists() {
            return Err(PresetError::Taken(new.to_string()));
        }
        p.name = new.to_string();
        if same_file && from != to {
            // A case-only change: rename the file itself, so the disk shows the new spelling.
            std::fs::rename(&from, &to)?;
        }
        self.save(&p)?;
        if !same_file {
            std::fs::remove_file(&from)?;
        }
        Ok(p)
    }

    /// Move the preset `name` from `folder` into `to` (`None`: no folder). Returns it moved.
    ///
    /// ⚠ A name the target folder already holds is refused, never overwritten.
    pub fn move_to(
        &self,
        folder: Option<&str>,
        name: &str,
        to: Option<&str>,
    ) -> Result<Preset, PresetError> {
        if folder == to {
            return self
                .get(folder, name)
                .ok_or_else(|| PresetError::Missing(name.to_string()));
        }
        let mut p = self
            .get(folder, name)
            .ok_or_else(|| PresetError::Missing(name.to_string()))?;
        if self.exists(to, name) {
            return Err(PresetError::Taken(name.to_string()));
        }
        p.folder = to.map(str::to_string);
        self.save(&p)?;
        if let Some(from) = self.path_of(folder, name) {
            std::fs::remove_file(from)?;
        }
        Ok(p)
    }

    /// Remove a preset's file outright — no trash. For an undo taking back what it made.
    pub fn remove(&self, folder: Option<&str>, name: &str) -> Result<(), PresetError> {
        if let Some(p) = self.path_of(folder, name) {
            match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Make a folder; returns the name it was made under (cleaned by `safe_name::folder_name`).
    ///
    /// ⚠ A name a folder already has is refused — a case-insensitive disk would otherwise make one
    /// folder of two.
    pub fn create_folder(&self, typed: &str) -> Result<String, PresetError> {
        let name = clean_folder(typed)?;
        let dir = self.dir.join(&name);
        if dir.exists() || self.folders().iter().any(|f| f.eq_ignore_ascii_case(&name)) {
            return Err(PresetError::Taken(name));
        }
        std::fs::create_dir_all(dir)?;
        Ok(name)
    }

    /// Rename a folder; returns the new name. A case-only change is allowed; a taken name is not.
    pub fn rename_folder(&self, old: &str, typed: &str) -> Result<String, PresetError> {
        let name = clean_folder(typed)?;
        let taken = self
            .folders()
            .iter()
            .any(|f| f != old && f.eq_ignore_ascii_case(&name));
        if taken {
            return Err(PresetError::Taken(name));
        }
        if name != old {
            std::fs::rename(self.dir.join(old), self.dir.join(&name))?;
        }
        Ok(name)
    }

    /// Delete a folder: each of its presets to the trash, then the directory. Returns the presets.
    pub fn remove_folder(&self, name: &str) -> Result<Vec<Preset>, PresetError> {
        let presets = read_dir_presets(&self.dir.join(name), Some(name));
        for p in &presets {
            self.delete(Some(name), &p.name)?;
        }
        self.remove_empty_folder(name);
        Ok(presets)
    }

    /// Make sure a folder's directory exists — an undo putting a folder back.
    pub fn ensure_folder(&self, name: &str) -> Result<(), PresetError> {
        std::fs::create_dir_all(self.dir.join(name))?;
        Ok(())
    }

    /// Remove a folder's directory if nothing is left in it — never a folder that still holds a file.
    pub fn remove_empty_folder(&self, name: &str) {
        let _ = std::fs::remove_dir(self.dir.join(name));
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

    /// The newest readable trash entry matching `want` (a folder and a name), with its path.
    /// `None` for `want` means "any". Malformed entries are skipped, the way [`Self::list`] skips
    /// them.
    fn latest_trashed(&self, want: Option<(Option<&str>, &str)>) -> Option<(PathBuf, Preset)> {
        self.trash_files().into_iter().rev().find_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let (folder, mut preset) = read_trash_entry(&text)?;
            match want {
                Some((f, name)) if preset.name != name || folder.as_deref() != f => None,
                _ => {
                    preset.folder = folder;
                    Some((path, preset))
                }
            }
        })
    }

    /// The name of the newest trashed preset.
    pub fn latest_trashed_name(&self) -> Option<String> {
        self.latest_trashed(None).map(|(_, p)| p.name)
    }

    /// Restore the newest trashed preset **named `name` from `folder`** into that folder. `Ok(None)`
    /// if there is none.
    ///
    /// By name rather than "whatever is newest" so that undoing a specific deletion stays
    /// correct however the trash was reordered by other deletions in between.
    ///
    /// Restoring over a live preset of the same name **overwrites** it, consistent with
    /// [`Self::save`]. Returns what was restored, for the status line.
    pub fn restore(&self, folder: Option<&str>, name: &str) -> Result<Option<Preset>, PresetError> {
        let Some((path, preset)) = self.latest_trashed(Some((folder, name))) else {
            return Ok(None);
        };
        self.save(&preset)?;
        let _ = std::fs::remove_file(&path);
        Ok(Some(preset))
    }
}

/// The presets directly in `dir`, each given `folder`, sorted by name.
fn read_dir_presets(dir: &Path, folder: Option<&str>) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Preset> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| read_preset_file(&p))
        .map(|mut p| {
            p.folder = folder.map(str::to_string);
            p
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A preset read from any `.json` file — what Import reads. `None` for anything that is not one.
pub fn read_preset_file(path: &Path) -> Option<Preset> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Preset>(&text).ok()
}

/// Two preset paths that are one file: equal, or differing only in case **and** the same file on
/// disk — a case-insensitive disk's one name. ⚠ On a case-sensitive disk `warm.json` beside
/// `Warm.json` is a different preset, and treating it as the same would overwrite it.
fn same_name(a: &Path, b: &Path) -> bool {
    a == b
        || (a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
            && one_file(a, b))
}

#[cfg(unix)]
fn one_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.ino() == y.ino() && x.dev() == y.dev(),
        _ => false,
    }
}

/// ⚠ Windows' disks are case-insensitive: two names differing in case that both exist are one file.
#[cfg(not(unix))]
fn one_file(a: &Path, b: &Path) -> bool {
    a.exists() && b.exists()
}

fn clean_folder(typed: &str) -> Result<String, PresetError> {
    let name = folder_name(typed);
    if name.is_empty() || name.starts_with('.') {
        return Err(PresetError::EmptyName);
    }
    Ok(name)
}

/// `wanted`, or — when a name among `taken` is the same file (compared sanitized, case aside) —
/// `wanted 2`, `wanted 3`, … Import's rule for a name a folder already holds.
pub fn unique_name(taken: &[String], wanted: &str) -> String {
    let key = |n: &str| sanitize(n).to_lowercase();
    let used: Vec<String> = taken.iter().map(|t| key(t)).collect();
    let wanted = wanted.trim();
    (1..)
        .map(|n| {
            if n == 1 {
                wanted.to_string()
            } else {
                format!("{wanted} {n}")
            }
        })
        .find(|c| !used.contains(&key(c)))
        .expect("an unbounded search always ends")
}

/// Where Export writes `stem.ext` in `dir` — `stem-1.ext`, `stem-2.ext`, … when the name is taken.
/// ⚠ Never a path that exists: an export does not replace a file.
pub fn export_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let named = |n: usize| {
        let s = if n == 0 {
            stem.to_string()
        } else {
            format!("{stem}-{n}")
        };
        if ext.is_empty() {
            dir.join(s)
        } else {
            dir.join(format!("{s}.{ext}"))
        }
    };
    (0..)
        .map(named)
        .find(|p| !p.exists())
        .expect("an unbounded search always ends")
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{EditGroups, Edits};

    /// A unique temp directory for one test, removed on drop. Keeps tests off the real
    /// home directory without adding a `tempfile` dependency.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            Self(test_dir::path(&format!("presets-{tag}")))
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
            folder: None,
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

        store.delete(None, "Punchy").unwrap();
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
        // ⚠ Into `presets/` itself — this wrote beside it once, where `list` never looks, so the
        // test could not fail (found moving it, M10.9a).
        std::fs::write(dir.0.join("presets/broken.json"), "{ not json").unwrap();
        std::fs::write(dir.0.join("presets/notes.txt"), "ignore me").unwrap();

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
        let base = test_dir::path("presets-absent-dir");
        let store = PresetStore::new(base.join("presets"), base.join("trash"));
        assert!(store.list().is_empty());
        assert_eq!(store.latest_trashed_name(), None);
    }

    #[test]
    fn deleting_moves_the_preset_to_the_trash() {
        let dir = TempDir::new("to_trash");
        let store = dir.store();
        store.save(&preset("Punchy", 1.0)).unwrap();
        store.delete(None, "Punchy").unwrap();

        assert!(store.list().is_empty(), "still live after delete");
        assert_eq!(dir.trash_count(), 1);
        assert_eq!(store.latest_trashed_name().as_deref(), Some("Punchy"));
    }

    #[test]
    fn restore_brings_it_back_and_empties_that_slot() {
        let dir = TempDir::new("undo");
        let store = dir.store();
        store.save(&preset("Punchy", 1.5)).unwrap();
        store.delete(None, "Punchy").unwrap();

        let restored = store
            .restore(None, "Punchy")
            .unwrap()
            .expect("something to restore");
        assert_eq!(restored.name, "Punchy");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].edits.exposure, 1.5);
        assert_eq!(dir.trash_count(), 0, "the entry should leave the trash");
        assert_eq!(store.latest_trashed_name(), None);
    }

    #[test]
    fn restoring_from_an_empty_trash_is_ok_none() {
        let dir = TempDir::new("undo_empty");
        assert!(dir.store().restore(None, "Nothing").unwrap().is_none());
    }

    #[test]
    fn deleting_the_same_name_twice_keeps_both_copies() {
        // ⚠ Without the timestamp prefix the second deletion would clobber the first
        // trashed copy, and the older version would be gone with no sign of it.
        let dir = TempDir::new("same_name");
        let store = dir.store();
        store.save(&preset("Look", 1.0)).unwrap();
        store.delete(None, "Look").unwrap();
        store.save(&preset("Look", 2.0)).unwrap();
        store.delete(None, "Look").unwrap();

        assert_eq!(dir.trash_count(), 2);
        // Newest first: the second version comes back before the first.
        assert_eq!(
            store.restore(None, "Look").unwrap().unwrap().edits.exposure,
            2.0
        );
        assert_eq!(
            store.restore(None, "Look").unwrap().unwrap().edits.exposure,
            1.0
        );
    }

    #[test]
    fn deletions_within_one_millisecond_both_survive() {
        // ⚠ This loop runs far faster than the millisecond clock ticks, so without the
        // collision bump in `trash_path` most of these would overwrite each other.
        let dir = TempDir::new("same_ms");
        let store = dir.store();
        for i in 0..8 {
            store.save(&preset(&format!("P{i}"), i as f32)).unwrap();
            store.delete(None, &format!("P{i}")).unwrap();
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
            store.delete(None, &format!("P{i:03}")).unwrap();
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
        while let Some(next) = store.latest_trashed_name() {
            let p = store.restore(None, &next).unwrap().expect("just saw it");
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
        store.delete(None, "Look").unwrap();
        store.save(&preset("Look", 9.0)).unwrap();

        store.restore(None, "Look").unwrap().unwrap();
        let got = store.list();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].edits.exposure, 1.0, "the trashed version should win");
    }

    #[test]
    fn a_malformed_trash_entry_is_skipped_not_fatal() {
        let dir = TempDir::new("bad_trash");
        let store = dir.store();
        store.save(&preset("Good", 1.0)).unwrap();
        store.delete(None, "Good").unwrap();
        // Sorts after the real entry, so it is the first one `latest_trashed` examines.
        std::fs::write(dir.0.join("trash").join("9999999999999-x.json"), "{ nope").unwrap();

        assert_eq!(store.latest_trashed_name().as_deref(), Some("Good"));
        assert_eq!(store.restore(None, "Good").unwrap().unwrap().name, "Good");
    }

    #[test]
    fn restoring_targets_the_named_preset_not_merely_the_newest() {
        // What restore-by-name buys: the action log may ask for a deletion that other
        // deletions have since buried, and it must still get the right one back.
        let dir = TempDir::new("by_name");
        let store = dir.store();
        for n in ["A", "B", "C"] {
            store.save(&preset(n, 1.0)).unwrap();
            store.delete(None, n).unwrap();
        }
        let got = store
            .restore(None, "A")
            .unwrap()
            .expect("A should be recoverable");
        assert_eq!(got.name, "A");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].name, "A");
        // B and C are untouched and still recoverable.
        assert_eq!(dir.trash_count(), 2);
        assert_eq!(store.restore(None, "C").unwrap().unwrap().name, "C");
    }

    #[test]
    fn restoring_a_name_that_is_not_in_the_trash_is_ok_none() {
        let dir = TempDir::new("absent_name");
        let store = dir.store();
        store.save(&preset("A", 1.0)).unwrap();
        store.delete(None, "A").unwrap();
        assert!(store.restore(None, "B").unwrap().is_none());
        assert_eq!(dir.trash_count(), 1, "the wrong lookup must not consume A");
    }

    #[test]
    fn deleting_something_that_is_already_gone_is_not_an_error() {
        let dir = TempDir::new("gone");
        let store = dir.store();
        store.delete(None, "Nothing").unwrap();
        assert_eq!(dir.trash_count(), 0, "nothing should be trashed");
    }
}

/// M10.9a: folders, rename, move, the folder-aware trash, and the naming rules.
#[cfg(test)]
mod folder_tests {
    use super::*;
    use edit_model::{EditGroups, Edits};

    fn store(d: &test_dir::TempDir) -> PresetStore {
        PresetStore::new(d.join("presets"), d.join("trash"))
    }

    fn preset(folder: Option<&str>, name: &str, exposure: f32) -> Preset {
        Preset {
            name: name.to_string(),
            groups: EditGroups::default(),
            edits: Edits {
                exposure,
                ..Edits::default()
            },
            folder: folder.map(str::to_string),
        }
    }

    fn listed(s: &PresetStore) -> Vec<(Option<String>, String)> {
        s.list().into_iter().map(|p| (p.folder, p.name)).collect()
    }

    fn at(folder: Option<&str>, name: &str) -> (Option<String>, String) {
        (folder.map(str::to_string), name.to_string())
    }

    #[test]
    fn presets_list_by_folder_and_empty_folders_count() {
        let d = test_dir::TempDir::new("preset-folders");
        let s = store(&d);
        s.save(&preset(None, "Zed", 0.0)).unwrap();
        s.save(&preset(Some("Film"), "Warm", 1.0)).unwrap();
        s.save(&preset(Some("Film"), "Cool", 1.0)).unwrap();
        s.create_folder("Empty").unwrap();
        std::fs::create_dir_all(d.join("presets/.hidden")).unwrap();
        assert_eq!(
            listed(&s),
            [
                at(None, "Zed"),
                at(Some("Film"), "Cool"),
                at(Some("Film"), "Warm")
            ]
        );
        assert_eq!(s.folders(), ["Empty", "Film"]);
        assert!(
            d.join("presets/Film/Warm.json").exists(),
            "a folder is a directory"
        );
        let text = std::fs::read_to_string(d.join("presets/Film/Warm.json")).unwrap();
        assert!(
            !text.contains("folder"),
            "the file format is unchanged: {text}"
        );
    }

    #[test]
    fn the_key_names_the_folder() {
        assert_eq!(preset(None, "Warm", 0.0).key(), "Warm");
        assert_eq!(preset(Some("Film"), "Warm", 0.0).key(), "Film/Warm");
    }

    /// ⚠⚠ The trash remembers the folder, so a restore puts the preset back where it was; an entry
    /// written before folders existed still reads, as no folder.
    #[test]
    fn the_trash_remembers_the_folder_and_reads_an_old_entry() {
        let d = test_dir::TempDir::new("preset-trash-folder");
        let s = store(&d);
        s.save(&preset(Some("Film"), "Warm", 2.0)).unwrap();
        s.delete(Some("Film"), "Warm").unwrap();
        assert!(listed(&s).is_empty(), "the sample: gone");
        assert!(
            s.restore(None, "Warm").unwrap().is_none(),
            "not the top level's"
        );
        let back = s.restore(Some("Film"), "Warm").unwrap().unwrap();
        assert_eq!(back.folder.as_deref(), Some("Film"));
        assert_eq!(listed(&s), [at(Some("Film"), "Warm")]);

        std::fs::create_dir_all(d.join("trash")).unwrap();
        let old = serde_json::to_string(&preset(None, "Old", 1.0)).unwrap();
        std::fs::write(d.join("trash/0000000000001-Old.json"), old).unwrap();
        assert_eq!(s.restore(None, "Old").unwrap().unwrap().name, "Old");
    }

    /// ⚠⚠ A rename or a move never overwrites: a taken name is refused and both files stay.
    #[test]
    fn rename_and_move_refuse_a_taken_name() {
        let d = test_dir::TempDir::new("preset-taken");
        let s = store(&d);
        s.save(&preset(None, "A", 1.0)).unwrap();
        s.save(&preset(None, "B", 2.0)).unwrap();
        s.save(&preset(Some("Film"), "A", 3.0)).unwrap();
        assert!(matches!(
            s.rename(None, "A", "B"),
            Err(PresetError::Taken(_))
        ));
        assert!(matches!(
            s.move_to(None, "A", Some("Film")),
            Err(PresetError::Taken(_))
        ));
        assert_eq!(s.get(None, "B").unwrap().edits.exposure, 2.0, "B untouched");
        assert_eq!(s.get(Some("Film"), "A").unwrap().edits.exposure, 3.0);
        assert_eq!(
            s.get(None, "A").unwrap().edits.exposure,
            1.0,
            "A still there"
        );

        let r = s.rename(None, "A", "C").unwrap();
        assert_eq!(r.name, "C");
        assert!(!s.exists(None, "A") && s.get(None, "C").unwrap().edits.exposure == 1.0);
        let m = s.move_to(None, "C", Some("Film")).unwrap();
        assert_eq!(m.folder.as_deref(), Some("Film"));
        assert!(!s.exists(None, "C") && s.exists(Some("Film"), "C"));
    }

    /// A change of case only is a rename of the same preset, not a collision with itself.
    #[test]
    fn a_change_of_case_renames_in_place() {
        let d = test_dir::TempDir::new("preset-case");
        let s = store(&d);
        s.save(&preset(None, "warm", 1.0)).unwrap();
        s.rename(None, "warm", "Warm").unwrap();
        assert_eq!(listed(&s), [at(None, "Warm")]);
    }

    #[test]
    fn folders_are_created_renamed_and_removed_into_the_trash() {
        let d = test_dir::TempDir::new("preset-folder-ops");
        let s = store(&d);
        assert_eq!(s.create_folder(" Film ").unwrap(), "Film");
        assert!(matches!(
            s.create_folder("film"),
            Err(PresetError::Taken(_))
        ));
        assert!(matches!(s.create_folder("  "), Err(PresetError::EmptyName)));
        s.save(&preset(Some("Film"), "Warm", 1.0)).unwrap();
        s.create_folder("Other").unwrap();
        assert!(matches!(
            s.rename_folder("Film", "other"),
            Err(PresetError::Taken(_))
        ));
        assert_eq!(s.rename_folder("Film", "Analog").unwrap(), "Analog");
        assert_eq!(listed(&s), [at(Some("Analog"), "Warm")]);

        let gone = s.remove_folder("Analog").unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(s.folders(), ["Other"]);
        assert!(
            s.restore(Some("Analog"), "Warm").unwrap().is_some(),
            "in the trash"
        );
        assert_eq!(
            listed(&s),
            [at(Some("Analog"), "Warm")],
            "and back in its folder"
        );
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let taken = vec!["Warm".to_string(), "warm 2".to_string()];
        assert_eq!(unique_name(&taken, "Cool"), "Cool");
        assert_eq!(unique_name(&taken, "Warm"), "Warm 3");
        assert_eq!(unique_name(&taken, " warm "), "warm 3");
    }

    /// ⚠ An export never replaces a file.
    #[test]
    fn an_export_path_is_never_taken() {
        let d = test_dir::TempDir::new("preset-export-path");
        std::fs::write(d.join("Warm.json"), "x").unwrap();
        std::fs::write(d.join("Warm-1.json"), "x").unwrap();
        assert_eq!(export_path(d.path(), "Warm", "json"), d.join("Warm-2.json"));
        assert_eq!(export_path(d.path(), "Cool", "json"), d.join("Cool.json"));
        assert_eq!(export_path(d.path(), "Film", ""), d.join("Film"));
    }
}
