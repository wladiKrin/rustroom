# Presets & Copy/Paste Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move an `Edits` around — copy/paste between photos, and named presets saved to disk — with per-group control over which settings transfer.

**Architecture:** One new type, `EditGroups`, partitions `Edits` into six groups and drives `Edits::merge_from`. Two new `app` lib modules own the disk side (`PresetStore`, `SettingsStore`), both with injected paths so they are unit-testable against temp directories. `main.rs` gains only rendering: a left preset panel and four small modals. **No pipeline stage is touched**, so the 23 goldens must stay byte-identical.

**Tech Stack:** Rust workspace, `serde`/`serde_json`, `thiserror`, `dirs` (new), `egui`/`eframe` 0.35.

**Spec:** `docs/superpowers/specs/2026-08-07-presets-copy-paste-design.md` (approved — do not re-derive).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/edit-model/src/lib.rs` | `EditGroups`, `EDIT_GROUP_NAMES`, `Edits::merge_from` | Modify |
| `Cargo.toml` | `dirs` in `[workspace.dependencies]` | Modify |
| `crates/app/Cargo.toml` | `serde`, `serde_json`, `thiserror`, `dirs` | Modify |
| `crates/app/src/presets.rs` | `Preset`, `PresetStore`, name sanitizing | **Create** |
| `crates/app/src/settings.rs` | `Settings`, `SettingsStore`, `ConfirmOutcome` | **Create** |
| `crates/app/src/lib.rs` | register both modules | Modify |
| `crates/app/src/main.rs` | app state, shortcuts, left panel, four modals | Modify |
| `CLAUDE.md` | status section | Modify |

Every command below assumes:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Note on package names:** the GUI crate is package `app`, lib `rustroom_app`, binary `Rustroom`. Use `cargo test -p app`, never `-p rustroom-app`.

---

## Task 1: `EditGroups` and `Edits::merge_from`

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block in `crates/edit-model/src/lib.rs`:

```rust
    /// An `Edits` with every field non-neutral, so a field that fails to transfer shows up.
    ///
    /// Deliberately does NOT use `..Default::default()`: adding a field to `Edits` should
    /// break this function, forcing whoever adds it to give it a group.
    fn all_nonneutral() -> Edits {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            version: 7,
            white_balance: WhiteBalance {
                temp: 10.0,
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            sharpening: Sharpening {
                amount: 60.0,
                ..Default::default()
            },
            vignette: Vignette {
                amount: -30.0,
                ..Default::default()
            },
            geometry: Geometry {
                angle: 2.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
            },
        }
    }

    #[test]
    fn merging_all_groups_reproduces_the_source() {
        // ⚠ The coverage half of the partition proof. A field belonging to NO group would
        // silently never transfer — and this is what catches it, because the result would
        // keep the target's default for that field.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got, Edits { version: 0, ..src });
    }

    #[test]
    fn version_never_transfers() {
        // `version` is a schema marker, not an edit. The target keeps its own.
        let src = all_nonneutral();
        assert_ne!(src.version, 0);
        let mut got = Edits {
            version: 3,
            ..Edits::default()
        };
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got.version, 3);
    }

    #[test]
    fn merging_no_groups_changes_nothing() {
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::NONE);
        assert_eq!(got, Edits::default());
    }

    #[test]
    fn every_group_transfers_something() {
        // A group that moves no fields is dead weight, and would make the disjointness
        // test below pass vacuously for that group.
        let src = all_nonneutral();
        for i in 0..EDIT_GROUP_NAMES.len() {
            let mut got = Edits::default();
            got.merge_from(&src, EditGroups::only(i));
            assert_ne!(
                got,
                Edits::default(),
                "group {} transferred nothing",
                EDIT_GROUP_NAMES[i]
            );
        }
    }

    #[test]
    fn the_groups_are_disjoint() {
        // ⚠ The other half of the partition proof, and it needs no field list of its own.
        //
        // Clear group `g` from a fully non-neutral source. If some field belonged to both
        // `g` and `h`, clearing `g` would change what `h` transfers — so comparing group
        // `h`'s transfer before and after catches any overlap generically.
        let src = all_nonneutral();
        let n = EDIT_GROUP_NAMES.len();
        for g in 0..n {
            let mut cleared = src.clone();
            cleared.merge_from(&Edits::default(), EditGroups::only(g));
            for h in 0..n {
                if h == g {
                    continue;
                }
                let mut from_src = Edits::default();
                from_src.merge_from(&src, EditGroups::only(h));
                let mut from_cleared = Edits::default();
                from_cleared.merge_from(&cleared, EditGroups::only(h));
                assert_eq!(
                    from_src, from_cleared,
                    "clearing {} changed what {} transfers — the groups overlap",
                    EDIT_GROUP_NAMES[g], EDIT_GROUP_NAMES[h]
                );
            }
        }
    }

    #[test]
    fn clearing_then_restoring_a_group_is_a_round_trip() {
        // Proves group `g` covers *exactly* the fields that `cleared` lost — not fewer.
        let src = all_nonneutral();
        for g in 0..EDIT_GROUP_NAMES.len() {
            let mut round = src.clone();
            round.merge_from(&Edits::default(), EditGroups::only(g));
            assert_ne!(round, src, "group {} did nothing", EDIT_GROUP_NAMES[g]);
            round.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                round, src,
                "group {} did not restore everything it cleared",
                EDIT_GROUP_NAMES[g]
            );
        }
    }

    #[test]
    fn the_default_selection_excludes_geometry() {
        // A crop framed for one composition reframes another, and a preset built on a 3:2
        // crop would reshape every photo it touches.
        let g = EditGroups::default();
        assert!(!g.geometry);
        assert!(g.white_balance && g.tone && g.color && g.detail && g.effects);
    }

    #[test]
    fn edit_groups_roundtrip_json_and_default_when_absent() {
        let g = EditGroups {
            geometry: true,
            tone: false,
            ..EditGroups::default()
        };
        let back: EditGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(g, back);
        // An older file missing the field entirely still loads.
        let partial: EditGroups = serde_json::from_str(r#"{"geometry": true}"#).unwrap();
        assert!(partial.geometry);
        assert_eq!(partial.tone, EditGroups::default().tone);
    }

    #[test]
    fn group_accessors_match_the_name_table() {
        let mut g = EditGroups::NONE;
        for i in 0..EDIT_GROUP_NAMES.len() {
            assert!(!g.get(i));
            *g.get_mut(i) = true;
            assert!(g.get(i));
        }
        assert_eq!(g, EditGroups::ALL);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model merge
```

Expected: FAIL to compile — `cannot find type 'EditGroups'`.

- [ ] **Step 3: Add the type**

In `crates/edit-model/src/lib.rs`, insert immediately **before** the `/// All edits applied to one image.` doc comment:

```rust
/// Group names in [`EditGroups`] field order — the one source of truth shared by the merge
/// logic, the checkbox UI and the tests. Mirrors [`HSL_BAND_NAMES`].
pub const EDIT_GROUP_NAMES: [&str; 6] = [
    "White Balance",
    "Tone",
    "Color",
    "Detail",
    "Effects",
    "Geometry",
];

/// Which groups of settings a paste or a preset carries.
///
/// [`Default`] is everything **except** geometry: a crop framed for one composition reframes
/// another, so carrying it across photos is usually wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditGroups {
    pub white_balance: bool,
    /// Exposure, contrast, highlights, shadows, whites, blacks, tone curve.
    pub tone: bool,
    /// Saturation, vibrance, HSL.
    pub color: bool,
    /// Noise reduction and sharpening.
    pub detail: bool,
    /// Vignette.
    pub effects: bool,
    /// Crop and straighten.
    pub geometry: bool,
}

impl Default for EditGroups {
    fn default() -> Self {
        Self {
            white_balance: true,
            tone: true,
            color: true,
            detail: true,
            effects: true,
            geometry: false,
        }
    }
}

impl EditGroups {
    pub const ALL: Self = Self {
        white_balance: true,
        tone: true,
        color: true,
        detail: true,
        effects: true,
        geometry: true,
    };

    pub const NONE: Self = Self {
        white_balance: false,
        tone: false,
        color: false,
        detail: false,
        effects: false,
        geometry: false,
    };

    /// Only group `i`, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn only(i: usize) -> Self {
        let mut g = Self::NONE;
        *g.get_mut(i) = true;
        g
    }

    /// Whether group `i` is selected, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get(&self, i: usize) -> bool {
        match i {
            0 => self.white_balance,
            1 => self.tone,
            2 => self.color,
            3 => self.detail,
            4 => self.effects,
            5 => self.geometry,
            _ => panic!("edit group index {i} out of range"),
        }
    }

    /// Mutable access to group `i`, for the checkbox UI. Indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get_mut(&mut self, i: usize) -> &mut bool {
        match i {
            0 => &mut self.white_balance,
            1 => &mut self.tone,
            2 => &mut self.color,
            3 => &mut self.detail,
            4 => &mut self.effects,
            5 => &mut self.geometry,
            _ => panic!("edit group index {i} out of range"),
        }
    }
}
```

- [ ] **Step 4: Add `merge_from`**

In the existing `impl Edits { ... }` block in the same file, add before the closing brace:

```rust
    /// Overwrite this photo's settings with `src`'s, for the selected groups only.
    ///
    /// `version` is a schema marker rather than an edit, so it never transfers — the target
    /// keeps its own.
    ///
    /// ⚠ The destructuring below is **exhaustive on purpose**. Adding a field to `Edits`
    /// breaks this function until whoever adds it decides which group owns it. Without that,
    /// a new field would silently never transfer through a paste or a preset, and nothing
    /// would fail — the same class of bug `PrefixKey` guards against in the pipeline.
    pub fn merge_from(&mut self, src: &Edits, groups: EditGroups) {
        let Edits {
            version: _,
            white_balance,
            exposure,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            tone_curve,
            saturation,
            vibrance,
            hsl,
            noise_reduction,
            sharpening,
            vignette,
            geometry,
        } = src;

        if groups.white_balance {
            self.white_balance = *white_balance;
        }
        if groups.tone {
            self.exposure = *exposure;
            self.contrast = *contrast;
            self.highlights = *highlights;
            self.shadows = *shadows;
            self.whites = *whites;
            self.blacks = *blacks;
            self.tone_curve = tone_curve.clone();
        }
        if groups.color {
            self.saturation = *saturation;
            self.vibrance = *vibrance;
            self.hsl = *hsl;
        }
        if groups.detail {
            self.noise_reduction = *noise_reduction;
            self.sharpening = *sharpening;
        }
        if groups.effects {
            self.vignette = *vignette;
        }
        if groups.geometry {
            self.geometry = *geometry;
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p edit-model
```

Expected: PASS, all 36 tests (28 pre-existing + 8 new).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p edit-model
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): add EditGroups and Edits::merge_from"
```

---

## Task 2: `PresetStore`

**Files:**
- Modify: `Cargo.toml`, `crates/app/Cargo.toml`
- Create: `crates/app/src/presets.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Add the dependencies**

In the root `Cargo.toml`, add to `[workspace.dependencies]` after the `serde_json` line:

```toml
dirs = "5"
```

In `crates/app/Cargo.toml`, add to `[dependencies]` after the `anyhow` line:

```toml
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
dirs = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/presets.rs` containing **only** this test module for now:

```rust
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
        assert!(matches!(
            store.save(&preset("", 1.0)),
            Err(PresetError::EmptyName)
        ));
        assert!(matches!(
            store.save(&preset("///", 1.0)),
            Err(PresetError::EmptyName)
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn sanitize_keeps_readable_names_readable() {
        assert_eq!(sanitize("Warm Punchy 2"), "Warm Punchy 2");
        assert_eq!(sanitize("high-key_v2"), "high-key_v2");
        assert_eq!(sanitize("../../evil"), "______evil");
        assert_eq!(sanitize("a/b\\c"), "a_b_c");
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        // Nothing has been saved yet on a fresh install, so the directory does not exist.
        let store = PresetStore::new(std::env::temp_dir().join("rustroom_presets_absent_dir"));
        assert!(store.list().is_empty());
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/app/src/lib.rs`, add in alphabetical position (after `pub mod histogram;`... place it after `pub mod history;`):

```rust
pub mod presets;
```

- [ ] **Step 4: Run the tests to verify they fail**

```bash
cargo test -p app --lib presets
```

Expected: FAIL to compile — `cannot find type 'Preset'` / `'PresetStore'` / `'PresetError'`.

- [ ] **Step 5: Implement the store**

Prepend to `crates/app/src/presets.rs`, above the test module:

```rust
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
/// `../../evil` must not escape the presets directory. Replacing rather than stripping keeps
/// two distinct names from colliding on one file.
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
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p app --lib presets
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 9 tests PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add Cargo.toml Cargo.lock crates/app/Cargo.toml crates/app/src/presets.rs crates/app/src/lib.rs
git commit -m "feat(app): add PresetStore (one JSON file per preset)"
```

---

## Task 3: `SettingsStore` and the delete-confirm rule

**Files:**
- Create: `crates/app/src/settings.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/settings.rs` containing **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
        };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        // Forward compatibility: a file written by a newer build must still load.
        let f = TempFile::new("unknown");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false, "future_flag": 7}"#).unwrap();
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
```

- [ ] **Step 2: Register the module**

In `crates/app/src/lib.rs`, add after `pub mod render_engine;`:

```rust
pub mod settings;
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --lib settings
```

Expected: FAIL to compile — `cannot find type 'Settings'`.

- [ ] **Step 4: Implement it**

Prepend to `crates/app/src/settings.rs`, above the test module:

```rust
//! Persisted application preferences.
//!
//! Deliberately tiny. Other settings (`show_filmstrip`, panel widths, last folder, export
//! defaults) move in when they are actually wanted, not speculatively — egui panel sizes in
//! particular are fiddly to save and restore, and that belongs in whatever spec needs them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// User preferences that survive a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ask before deleting a preset. Cleared by "Don't ask again" in that dialog; restored
    /// from the Settings window.
    pub confirm_preset_delete: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_preset_delete: true,
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
        match outcome {
            ConfirmOutcome::Cancelled => false,
            ConfirmOutcome::Confirmed { suppress_future } => {
                if suppress_future {
                    self.confirm_preset_delete = false;
                }
                true
            }
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

    /// `~/.config/rustroom/settings.json` (platform equivalent elsewhere), beside the
    /// presets directory.
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
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib settings
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 8 tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/settings.rs crates/app/src/lib.rs
git commit -m "feat(app): add SettingsStore and the delete-confirm rule"
```

---

## Task 4: App state, copy/paste shortcuts and toolbar

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the imports and state**

In `crates/app/src/main.rs`, add to the `use` block at the top (after the `use rustroom_app::render_engine::...` line):

```rust
use edit_model::EditGroups;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::settings::{ConfirmOutcome, Settings, SettingsStore};
```

Add this enum immediately **before** `struct RustroomApp {`:

```rust
/// Which modal dialog is open. At most one at a time, so this is an enum rather than a
/// pile of booleans that could contradict each other.
#[derive(Default, PartialEq)]
enum Modal {
    #[default]
    None,
    /// Naming a new preset and choosing the groups it will carry.
    SavePreset { name: String, groups: EditGroups },
    /// Choosing which groups a paste applies.
    PasteSettings,
    /// Confirming a preset deletion. `dont_ask_again` is scratch state — it is committed to
    /// `Settings` only on the Delete path, never on Cancel.
    ConfirmDeletePreset { name: String, dont_ask_again: bool },
    Settings,
}
```

Add these fields to `struct RustroomApp`, after `show_filmstrip: bool,`:

```rust
    show_presets: bool,
    /// In-memory and per-session: the clipboard does not survive a restart and does not
    /// touch the OS clipboard.
    clipboard: Option<Edits>,
    /// Group selection last used for a paste, so Ctrl+V can skip the dialog.
    paste_groups: EditGroups,
    preset_store: PresetStore,
    /// Cached `preset_store.list()`, refreshed on save and delete rather than every frame.
    preset_list: Vec<Preset>,
    settings_store: SettingsStore,
    settings: Settings,
    modal: Modal,
```

In `RustroomApp::new`, replace the `Self { ... }` literal's tail so it reads (keeping the existing fields exactly as they are and adding these after `show_filmstrip: true,`):

```rust
            show_filmstrip: true,
            show_presets: false,
            clipboard: None,
            paste_groups: EditGroups::default(),
            preset_store,
            preset_list,
            settings_store,
            settings,
            modal: Modal::None,
```

and add these bindings at the top of `RustroomApp::new`, before the `Self {`:

```rust
        let preset_store = PresetStore::user_default();
        let preset_list = preset_store.list();
        let settings_store = SettingsStore::user_default();
        let settings = settings_store.load();
```

- [ ] **Step 2: Add the three helper methods**

Add these to the `impl RustroomApp` block that contains `persist_current_session` (immediately after that method):

```rust
    /// Apply `src`'s selected groups to the current photo: one undo step, one re-render.
    ///
    /// The single point of application for both paste and presets, so the two cannot drift.
    fn apply_edits_from(&mut self, src: &Edits, groups: EditGroups) {
        let Some(s) = &mut self.session else { return };
        s.edits.merge_from(src, groups);
        s.history.push(s.edits.clone());
        s.engine.request(
            preview_edits(&s.edits, s.crop_mode),
            s.render_max_dim(),
            s.requested_region,
        );
    }

    /// Re-read the presets directory. Called after a save or a delete, not every frame.
    fn reload_presets(&mut self) {
        self.preset_list = self.preset_store.list();
    }

    /// Persist the settings, surfacing a failure in the status line rather than silently.
    fn save_settings(&mut self) {
        if let Err(e) = self.settings_store.save(&self.settings) {
            self.status = format!("Could not save settings: {e}");
        }
    }
```

- [ ] **Step 3: Add the copy/paste shortcuts**

Add this method to the same `impl RustroomApp` block:

```rust
    /// Ctrl+C / Ctrl+V for the edit clipboard.
    ///
    /// Gated on nothing having keyboard focus, the way `handle_develop_arrows` already is,
    /// so Ctrl+C inside the preset-name field is not hijacked.
    fn handle_clipboard_keys(&mut self, ctx: &egui::Context) {
        if self.session.is_none() || self.modal != Modal::None {
            return;
        }
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        let (copy, paste) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::C) && i.modifiers.command,
                i.key_pressed(egui::Key::V) && i.modifiers.command,
            )
        });
        if copy {
            // `self.session.as_ref().map(..)` rather than `if let Some(s) = &self.session`:
            // the latter holds an immutable borrow of `self` across the assignment to
            // `self.clipboard` and does not compile.
            self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
            self.status = "Settings copied".to_string();
        }
        if paste {
            // Ctrl+V uses the last-used group selection with no dialog; the "Paste…"
            // button opens the chooser.
            if let Some(src) = self.clipboard.clone() {
                self.apply_edits_from(&src, self.paste_groups);
                self.status = "Settings pasted".to_string();
            }
        }
    }
```

Call it from `develop_body`, on the line immediately after `self.handle_develop_arrows(ctx);`:

```rust
        self.handle_clipboard_keys(ctx);
```

- [ ] **Step 4: Add the toolbar buttons**

In `develop_toolbar`, replace the trailing filmstrip block:

```rust
        if self.library.is_some() {
            ui.toggle_value(&mut self.show_filmstrip, "Filmstrip");
        }
```

with:

```rust
        if self.session.is_some() {
            ui.separator();
            if ui.button("Copy").clicked() {
                // Same borrow reason as in `handle_clipboard_keys`.
                self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
                self.status = "Settings copied".to_string();
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("Paste…"))
                .clicked()
            {
                self.modal = Modal::PasteSettings;
            }
            // Unlike the filmstrip, this does not need a Library: presets are just as
            // useful on a single-file open.
            ui.toggle_value(&mut self.show_presets, "Presets");
            if ui.button("Settings…").clicked() {
                self.modal = Modal::Settings;
            }
        }
        if self.library.is_some() {
            ui.toggle_value(&mut self.show_filmstrip, "Filmstrip");
        }
```

- [ ] **Step 5: Build and lint**

```bash
cargo build -p app
cargo clippy -p app --all-targets -- -D warnings
```

Expected: builds clean, no warnings. (The `Presets` toggle shows a panel that does not exist yet — that is Task 5.)

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): add the edit clipboard, preset/settings state and toolbar buttons"
```

---

## Task 5: The preset panel and the Save modal

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the panel**

Add this method to the `impl RustroomApp` block containing `develop_body`:

```rust
    /// The left preset panel: save the current edits as a preset, apply one, or delete one.
    fn preset_panel(&mut self, ui: &mut egui::Ui) {
        // egui borrows `self` through the closure, so the click is recorded here and acted
        // on after the panel closes.
        enum Action {
            None,
            Apply(usize),
            Delete(String),
            Save,
        }
        let mut action = Action::None;

        egui::Panel::left("presets")
            .default_size(180.0)
            .show(ui, |ui| {
                ui.heading("Presets");
                if ui.button("Save current as preset…").clicked() {
                    action = Action::Save;
                }
                ui.separator();
                if self.preset_list.is_empty() {
                    ui.label("No presets yet.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, p) in self.preset_list.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button("×").on_hover_text("Delete").clicked() {
                                action = Action::Delete(p.name.clone());
                            }
                            if ui.button(&p.name).clicked() {
                                action = Action::Apply(i);
                            }
                        });
                    }
                });
            });

        match action {
            Action::None => {}
            Action::Save => {
                self.modal = Modal::SavePreset {
                    name: String::new(),
                    groups: EditGroups::default(),
                };
            }
            Action::Apply(i) => {
                if let Some(p) = self.preset_list.get(i).cloned() {
                    self.apply_edits_from(&p.edits, p.groups);
                    self.status = format!("Applied preset “{}”", p.name);
                }
            }
            Action::Delete(name) => {
                if self.settings.confirm_preset_delete {
                    self.modal = Modal::ConfirmDeletePreset {
                        name,
                        dont_ask_again: false,
                    };
                } else {
                    self.delete_preset(&name);
                }
            }
        }
    }

    /// Delete a preset and refresh the cached list.
    fn delete_preset(&mut self, name: &str) {
        match self.preset_store.delete(name) {
            Ok(()) => self.status = format!("Deleted preset “{name}”"),
            Err(e) => self.status = format!("Could not delete preset: {e}"),
        }
        self.reload_presets();
    }
```

- [ ] **Step 2: Show it from `develop_body`**

In `develop_body`, insert this immediately **after** `self.handle_clipboard_keys(ctx);` and **before** the `egui::Panel::right("panel")` call — a left panel must be declared before the central content it shares space with:

```rust
        if self.show_presets && self.session.is_some() {
            self.preset_panel(ui);
        }
```

- [ ] **Step 3: Add the group-checkbox helper and the Save modal**

Add this free function near `draw_histogram` at the bottom of `main.rs`:

```rust
/// The six group checkboxes, shared by the Save-preset and Paste-settings dialogs.
fn edit_group_checkboxes(ui: &mut egui::Ui, groups: &mut EditGroups) {
    ui.horizontal(|ui| {
        if ui.button("All").clicked() {
            *groups = EditGroups::ALL;
        }
        if ui.button("None").clicked() {
            *groups = EditGroups::NONE;
        }
    });
    for (i, name) in edit_model::EDIT_GROUP_NAMES.iter().enumerate() {
        ui.checkbox(groups.get_mut(i), *name);
    }
}
```

Add this method to the `impl RustroomApp` block containing `preset_panel`:

```rust
    /// The "Save current as preset…" dialog.
    fn save_preset_modal(&mut self, ctx: &egui::Context) {
        let Modal::SavePreset { name, groups } = &mut self.modal else {
            return;
        };
        let mut name = std::mem::take(name);
        let mut groups = *groups;
        let mut close = false;
        let mut save = false;

        egui::Window::new("Save preset")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut name);
                });
                ui.separator();
                ui.label("Include:");
                edit_group_checkboxes(ui, &mut groups);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        if save {
            // Clone the edits *out* first: holding `&self.session` across the assignments
            // to `self.status` below would be an immutable borrow crossing a mutable one.
            if let Some(edits) = self.session.as_ref().map(|s| s.edits.clone()) {
                let preset = Preset {
                    name: name.trim().to_string(),
                    groups,
                    edits,
                };
                match self.preset_store.save(&preset) {
                    Ok(()) => self.status = format!("Saved preset “{}”", preset.name),
                    Err(e) => self.status = format!("Could not save preset: {e}"),
                }
                self.reload_presets();
            }
            close = true;
        }

        if close {
            self.modal = Modal::None;
        } else {
            self.modal = Modal::SavePreset { name, groups };
        }
    }
```

- [ ] **Step 4: Dispatch the modal**

Add this method to the same block:

```rust
    /// Draw whichever modal is open. Called once per frame from `ui`.
    fn draw_modals(&mut self, ctx: &egui::Context) {
        // Decide *which* modal first, then dispatch. Matching on `self.modal` directly and
        // calling a `&mut self` method from an arm keeps the scrutinee's borrow alive across
        // the call, which does not compile.
        enum Which {
            None,
            Save,
            Paste,
            ConfirmDelete,
            Settings,
        }
        let which = match &self.modal {
            Modal::None => Which::None,
            Modal::SavePreset { .. } => Which::Save,
            Modal::PasteSettings => Which::Paste,
            Modal::ConfirmDeletePreset { .. } => Which::ConfirmDelete,
            Modal::Settings => Which::Settings,
        };
        match which {
            Which::None => {}
            Which::Save => self.save_preset_modal(ctx),
            Which::Paste => self.paste_modal(ctx),
            Which::ConfirmDelete => self.confirm_delete_modal(ctx),
            Which::Settings => self.settings_modal(ctx),
        }
    }
```

In `fn ui`, add this as the **last** statement, after the `match self.view { ... }` block:

```rust
        self.draw_modals(&ctx);
```

- [ ] **Step 5: Build**

The three modal methods referenced by `draw_modals` land in Task 6, so this will not compile yet. Add temporary stubs to the same `impl` block, to be **replaced** in Task 6:

```rust
    fn paste_modal(&mut self, _ctx: &egui::Context) {}
    fn confirm_delete_modal(&mut self, _ctx: &egui::Context) {}
    fn settings_modal(&mut self, _ctx: &egui::Context) {}
```

```bash
cargo build -p app
cargo clippy -p app --all-targets -- -D warnings
```

Expected: builds clean, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): add the preset panel and the save-preset dialog"
```

---

## Task 6: The Paste, Delete-confirm and Settings modals

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Replace the three stubs**

In `crates/app/src/main.rs`, delete these three lines added in Task 5 Step 5:

```rust
    fn paste_modal(&mut self, _ctx: &egui::Context) {}
    fn confirm_delete_modal(&mut self, _ctx: &egui::Context) {}
    fn settings_modal(&mut self, _ctx: &egui::Context) {}
```

and put these in their place:

```rust
    /// The "Paste…" dialog: which groups this paste applies.
    ///
    /// The chosen selection is remembered in `paste_groups`, which is also what a bare
    /// Ctrl+V uses.
    fn paste_modal(&mut self, ctx: &egui::Context) {
        let mut groups = self.paste_groups;
        let mut close = false;
        let mut paste = false;

        egui::Window::new("Paste settings")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Apply:");
                edit_group_checkboxes(ui, &mut groups);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Paste").clicked() {
                        paste = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        self.paste_groups = groups;
        if paste {
            if let Some(src) = self.clipboard.clone() {
                self.apply_edits_from(&src, groups);
                self.status = "Settings pasted".to_string();
            }
            close = true;
        }
        if close {
            self.modal = Modal::None;
        }
    }

    /// The preset-delete confirmation.
    ///
    /// ⚠ "Don't ask again" is scratch state until Delete is clicked. Ticking it and then
    /// cancelling must leave the confirmation armed — see
    /// `settings::cancelling_never_suppresses_the_confirmation`, which is where that rule
    /// is actually tested.
    fn confirm_delete_modal(&mut self, ctx: &egui::Context) {
        let Modal::ConfirmDeletePreset {
            name,
            dont_ask_again,
        } = &mut self.modal
        else {
            return;
        };
        let name = std::mem::take(name);
        let mut dont_ask = *dont_ask_again;
        let mut outcome: Option<ConfirmOutcome> = None;

        egui::Window::new("Delete preset")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Delete the preset “{name}”?"));
                ui.checkbox(&mut dont_ask, "Don't ask again");
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        outcome = Some(ConfirmOutcome::Confirmed {
                            suppress_future: dont_ask,
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Some(ConfirmOutcome::Cancelled);
                    }
                });
            });

        match outcome {
            None => {
                // Still open — carry the scratch state to the next frame.
                self.modal = Modal::ConfirmDeletePreset {
                    name,
                    dont_ask_again: dont_ask,
                };
            }
            Some(o) => {
                let suppressed_before = self.settings.confirm_preset_delete;
                if self.settings.apply_delete_confirm(o) {
                    self.delete_preset(&name);
                }
                if self.settings.confirm_preset_delete != suppressed_before {
                    self.save_settings();
                }
                self.modal = Modal::None;
            }
        }
    }

    /// The Settings window. One setting today; a modal is the right weight for that.
    fn settings_modal(&mut self, ctx: &egui::Context) {
        let mut confirm = self.settings.confirm_preset_delete;
        let mut close = false;

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut confirm, "Confirm before deleting a preset");
                ui.separator();
                if ui.button("Close").clicked() {
                    close = true;
                }
            });

        if confirm != self.settings.confirm_preset_delete {
            self.settings.confirm_preset_delete = confirm;
            self.save_settings();
        }
        if close {
            self.modal = Modal::None;
        }
    }
```

- [ ] **Step 2: Build and lint**

```bash
cargo build -p app --release
cargo clippy -p app --all-targets -- -D warnings
```

Expected: builds clean, no warnings.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): add the paste, delete-confirm and settings dialogs"
```

---

## Task 7: Verify, then check it by hand

**Files:** none (verification only)

- [ ] **Step 1: Full workspace verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

Expected: all three green.

- [ ] **Step 2: Confirm the goldens are untouched**

```bash
cargo test -p pipeline --test golden
git status --short crates/pipeline/tests/golden
```

Expected: **23 tests pass** and `git status` prints **nothing** — the cheap proof that an
app-layer feature stayed in the app layer.

- [ ] **Step 3: Launch the GUI and exercise it**

```bash
cargo run --release -p app
```

Walk through, and report to the user what to look at:

1. **Presets** toggle appears in the toolbar with a photo open — including after `Open file…`
   with no folder loaded (unlike Filmstrip).
2. **Save current as preset…** → name it, untick Geometry, Save. It appears in the left panel.
3. Move some sliders, then **click the preset** — only the included groups change, and a
   single **Ctrl+Z** undoes the whole application.
4. **Ctrl+C** on one photo, arrow to another, **Ctrl+V** — settings transfer, crop does not.
5. **Paste…** opens the chooser; tick only Tone, Paste — only tone transfers, and the next
   bare Ctrl+V reuses that selection.
6. Click **×** on a preset → confirm dialog. **Tick "Don't ask again", then Cancel.** Click
   `×` again — **the dialog must still appear.** This is the trap; it is unit-tested, but
   confirm the wiring too.
7. Tick "Don't ask again" and click **Delete** — the next `×` deletes immediately.
8. **Settings…** → re-tick "Confirm before deleting a preset"; the dialog returns.
9. Quit and relaunch — presets and the settings flag both survive.
10. Type in the preset-name field and press **Ctrl+C** — it must copy *text*, not hijack into
    the edit clipboard.

- [ ] **Step 4: Confirm the files landed where expected**

```bash
ls ~/.config/rustroom/presets/ ~/.config/rustroom/settings.json
```

Expected: one `.json` per saved preset, plus the settings file.

---

## Task 8: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the crate layout**

In the crate-layout block, change the `edit-model/` line to mention `EditGroups`, and the
`app/` line to list the two new lib modules:

```
│   ├── edit-model/           # ✅ serde Edits op-list (WB/exposure/contrast/tone/curve/saturation/hsl/NR/sharpening/vignette/geometry) + EditGroups partition + merge_from
│   ├── app/                  # ✅ GUI `Rustroom` (eframe/egui 0.35 + wgpu). lib=history/crop_tool/histogram/render_engine/thumb_loader/decode_worker/buffer_cache/view_region/presets/settings
```

- [ ] **Step 2: Rewrite the RESUME HERE next-action**

Replace the "▶ NEXT ACTION" sentence and the numbered gap list with:

```markdown
**▶ NEXT ACTION: brainstorm multi-selection + sync**, then batch export — both want the same
Library selection model, and sync was deliberately cut out of the presets spec so that model
gets designed once, on its own terms. `Library.selected` is `Option<usize>` today.

**The remaining Phase 2 gaps:**
1. **Multi-selection + sync** — apply one photo's `Edits` to a selection. `EditGroups` and
   `Edits::merge_from` already exist and are tested, so sync itself is thin; the work is the
   selection model (ctrl-click, shift-click, select-all in both the grid and the filmstrip).
2. **Batch export** — reuses that same selection.
3. **Texture / clarity / dehaze** — local-contrast siblings of sharpening. `gaussian_blur`
   and the `RenderScale` seam already exist, so these are largely a different radius and a
   different curve on machinery that is built and tested.
```

- [ ] **Step 3: Add the status section**

Insert immediately above **"Sharpening done"**:

```markdown
**Presets & copy/paste done — moving an `Edits` around.** App-layer only: no pipeline stage was
touched, and all 23 goldens stayed byte-identical, which is the proof.

- **`EditGroups` (`edit-model`) partitions `Edits` into six groups** — White Balance / Tone /
  Color / Detail / Effects / Geometry — and drives `Edits::merge_from(&src, groups)`. One type
  serves both consumers, which is why presets and copy/paste are one feature and not two.
  `EDIT_GROUP_NAMES` + `get`/`get_mut`/`only(i)` mirror the `HSL_BAND_NAMES` pattern, so the
  checkbox UI and the tests index the same table.
- **`Default` is everything except Geometry**, deliberately: a crop framed for one composition
  reframes another, and a preset built on a 3:2 crop would reshape every photo it touches.
  `version` is a schema marker, not an edit, and never transfers.
- **⚠ `merge_from` destructures `Edits` exhaustively.** Adding a field is then a **compile
  error** until someone gives it a group — without that, a new field would silently never
  transfer and nothing would fail. Same class of bug as `PrefixKey`, but the guard is in
  production code rather than a test fixture.
- **The partition is tested as a partition, generically — no field lists.** Coverage:
  `merge(ALL) == src`, which alone catches an ungrouped field. Disjointness: clear group `g`
  from a fully non-neutral `Edits`, then check that what every *other* group transfers is
  unchanged — a field in two groups fails this without any test needing to know the field
  names. Plus a clear-then-restore round trip per group.
- **`app::presets`** — **one JSON file per preset** (`{ name, groups, edits }`), not one file
  holding all of them: a preset is then a shareable artifact, add/delete are plain file ops
  with no read-modify-write race, and one corrupt preset cannot take the rest down.
  **The preset stores its own group mask**, chosen at creation, so a "punchy tone" preset never
  disturbs white balance; copy/paste picks groups at *paste* time instead.
- **⚠ Filename sanitizing is load-bearing, not cosmetic** — the name is free text from the UI
  and becomes a path, so `../../evil` must not escape the directory. It is the one place in
  this feature where user input reaches the filesystem, and it has its own test. The display
  name is stored *inside* the file so it survives sanitizing.
- **`app::settings`** — Rustroom had **no settings system at all** before this; nothing
  survived a restart, not even `show_filmstrip`. Now `~/.config/rustroom/settings.json` holds
  one flag (`confirm_preset_delete`). **`load()` is infallible**: a missing *or malformed* file
  yields the defaults, because a corrupt settings file must never brick the app — the same
  forgiving posture the sidecars take. Saved on change, not on quit. Deliberately one field;
  others move in when actually wanted.
- **⚠ "Don't ask again" must only apply on Delete.** Tick it, think better of the deletion, hit
  Cancel — and the guard you just declined to use must still be armed. The rule lives in
  `Settings::apply_delete_confirm(ConfirmOutcome)`, a pure function separated from the egui
  code precisely so it is testable; `cancelling_never_suppresses_the_confirmation` pins it.
- **Both stores take their path by injection**, so tests run against temp directories and never
  touch the real home directory — the same pattern `DecodeWorker` uses for its decode fn. A
  local `TempDir`/`TempFile` guard with a `Drop` impl avoids adding a `tempfile` dependency.
- **UI:** a collapsible **left** panel (Lightroom's placement, and the right panel already
  scrolls through six sections). Unlike the Filmstrip toggle, **Presets shows without a
  `Library`** — it is just as useful on a single-file open. Ctrl+C copies; **Ctrl+V pastes with
  the last-used groups, no dialog**; "Paste…" opens the chooser. Both keys are gated on
  `ctx.memory(|m| m.focused())`, the way arrow-nav already is, so Ctrl+C in the name field is
  not hijacked. Applying anything is one `history.push` → **one undo step, for free**, because
  `History<Edits>` is snapshot-based. One `Modal` enum, so two dialogs cannot contradict.
- **New dependency:** `dirs` (cross-platform config dir). CLAUDE.md's "avoid platform-specific
  assumptions" rules out hand-rolling `$HOME`.
- **Accepted caveats:** (1) **a preset resets, it does not merge** — a preset whose Tone group
  is on but whose tone values are neutral will *reset* the target's tone. Correct (the preset
  asserts "tone is this"), matches Lightroom, and surprises people once. (2) No shipped
  presets; the panel starts empty. (3) Flat list, no folders or tags. (4) Not Lightroom
  compatible — no `.xmp`/`.lrtemplate`; presets are plain JSON copied by hand. (5) No rename —
  save under the new name and delete the old. (6) The clipboard is **in-memory and
  per-session**, and does not touch the OS clipboard. (7) Current photo only; multi-photo apply
  is what sync will add.
```

- [ ] **Step 4: Update the docs index**

In the `Docs (docs/superpowers/)` paragraph, add `2026-08-07-presets-copy-paste-design.md` to
the `specs/` list and `2026-08-07-presets-copy-paste.md` to the `plans/` list.

- [ ] **Step 5: Final verification**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --all
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-07-presets-copy-paste.md
git commit -m "docs: record presets and copy/paste"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical (`git status` clean under `tests/golden`).
- `merge_from` fails to compile if a field is added to `Edits` without being grouped.
- In the GUI: copy/paste by key and by dialog; save, apply and delete presets; the
  "Don't ask again" + Cancel case leaves the confirmation armed; presets and settings survive
  a restart.
- `CLAUDE.md` updated: `EditGroups`, the two new lib modules, the settings subsystem, and
  multi-selection + sync as the next spec.
