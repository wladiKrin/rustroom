# Export Presets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remember the last-used export options across restarts, and add named export presets — closing M2.4 and with it Phase 2.

**Architecture:** Two independent halves. Persistence puts `ExportOptions` into the existing `Settings`, which already loads at startup and is already infallible on a corrupt file. Named presets get an `ExportPresetStore` mirroring `PresetStore`'s one-file-per-preset shape, with the path-escape guard extracted into a shared `app::safe_name` so it exists once rather than twice. Everything is in `crates/app`; no pipeline, `edit-model` or stage is touched.

**Tech Stack:** Rust, `serde_json`, `dirs`, `egui` for one modal row.

**Spec:** `docs/superpowers/specs/2026-08-11-export-presets-design.md`

---

## Build environment

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠ Verify by hunting, not by running once.** This codebase has produced four flaky tests that a single run never showed — three of them from tests racing on shared temp paths, which is exactly what this feature's tests do:

```bash
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done
```

**⚠ CI is stricter than the usual local command**: it runs `cargo clippy --all-targets --all-features`, and `rust-toolchain.toml` says `channel = "stable"` unpinned, so CI can have newer lints.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/app/src/safe_name.rs` | **create** | `sanitize` — the one path-escape guard, shared |
| `crates/app/src/presets.rs` | modify | calls the shared `sanitize`; its private copy and that copy's tests move out |
| `crates/app/src/batch_export.rs` | modify | `ExportOptions` gains `Default` + serde; `destination_ready` |
| `crates/app/src/settings.rs` | modify | `Settings` gains `export: ExportOptions` |
| `crates/app/src/export_presets.rs` | **create** | `ExportPreset` + `ExportPresetStore` |
| `crates/app/src/lib.rs` | modify | declare the two new modules |
| `crates/app/src/main.rs` | modify | load/persist options; preset row in the export modal |
| `CLAUDE.md` | modify | status; M2.4 ✅ and Phase 2 complete |

---

## Task 1: Extract `safe_name`

**Files:**
- Create: `crates/app/src/safe_name.rs`
- Modify: `crates/app/src/presets.rs`, `crates/app/src/lib.rs`

- [ ] **Step 1: Create the module with the function and its tests**

Create `crates/app/src/safe_name.rs`:

```rust
//! Turning a user-typed name into a filename that cannot escape its directory.
//!
//! ⚠ Shared by `presets` and `export_presets` on purpose. This is the **single
//! security-relevant function** in either store — the thing standing between a preset called
//! `../../evil` and someone else's files — and duplicating such a guard is precisely how the
//! two copies drift and one of them becomes a real bug.

/// Map `name` to a filename stem containing only ASCII alphanumerics, space, `_` and `-`.
///
/// Returns an empty string when nothing usable survives, which callers must treat as a
/// rejection rather than writing a file called `.json`.
///
/// This is a **many-to-one** map, so `a/b` and `a_b` share a file and the later save wins —
/// see `distinct_names_can_still_collide_on_one_file` in `presets`. Accepted: hashing the
/// name would fix it at the cost of a directory nobody can read by hand, which is most of
/// the point of one file per preset.
pub fn sanitize(name: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names_stay_readable() {
        assert_eq!(sanitize("Warm Punchy 2"), "Warm Punchy 2");
        assert_eq!(sanitize("high-key_v2"), "high-key_v2");
    }

    #[test]
    fn a_traversal_attempt_cannot_escape_the_directory() {
        // ⚠ The reason this function exists, and why it is shared rather than copied.
        assert_eq!(sanitize("../../evil"), "______evil");
        assert_eq!(sanitize("a/b\\c"), "a_b_c");
        assert_eq!(sanitize("/etc/passwd"), "_etc_passwd");
        // Nothing that survives may contain a separator or a dot-segment.
        for probe in ["../x", "..", "./.", "a/../b", "C:\\win"] {
            let s = sanitize(probe);
            assert!(
                !s.contains('/') && !s.contains('\\') && !s.contains('.'),
                "{probe:?} sanitized to {s:?}"
            );
        }
    }

    #[test]
    fn a_name_with_nothing_usable_sanitizes_to_empty() {
        // Callers reject this rather than writing a file named ".json".
        for name in ["", "   ", "\t\n"] {
            assert!(sanitize(name).is_empty(), "{name:?}");
        }
        // But a name that is *all* replaced characters is a perfectly good filename.
        assert_eq!(sanitize("///"), "___");
    }
}
```

- [ ] **Step 2: Declare it**

In `crates/app/src/lib.rs`, add in alphabetical position (after `render_engine`):

```rust
pub mod safe_name;
```

- [ ] **Step 3: Point `presets` at it and delete the private copy**

In `crates/app/src/presets.rs`, add to the imports at the top:

```rust
use crate::safe_name::sanitize;
```

Then delete the private `fn sanitize(...)` and its doc comment entirely (the block beginning `/// `../../evil` must not escape the presets directory.`). Leave every call site alone — the name is unchanged.

In `presets.rs`'s `mod tests`, **delete** `fn sanitize_keeps_readable_names_readable` — it has moved to `safe_name`. Leave `a_name_that_sanitizes_to_nothing_is_rejected` and `distinct_names_can_still_collide_on_one_file` where they are: those test `PresetStore`'s *behaviour*, not the helper.

- [ ] **Step 4: Verify the extraction changed nothing**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib presets safe_name 2>&1 | tail -8
```

Expected: every pre-existing `presets` test passes untouched. **That is the acceptance criterion for this refactor** — the same role the goldens played for the guided-filter extraction.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "refactor(app): extract the filename guard into a shared safe_name module"
```

---

## Task 2: Persist the export options

**Files:**
- Modify: `crates/app/src/batch_export.rs`, `crates/app/src/settings.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/app/src/settings.rs`:

```rust
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
            export: crate::batch_export::ExportOptions {
                dest_dir: std::path::PathBuf::from("/tmp/rustroom_out"),
                quality: 71,
                max_edge: Some(2048),
            },
        };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
    }

    #[test]
    fn a_settings_file_from_before_export_options_still_loads() {
        // ⚠ The forgiving-load property, and the test most likely to be waved through as
        // obviously fine. Without `Default` on `ExportOptions` the `#[serde(default)]` on
        // `Settings` cannot fill the missing field, and every existing user's config would
        // fail to parse — which `load()` would silently turn into *all* settings reverting.
        let f = TempFile::new("export_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.export, crate::batch_export::ExportOptions::default());
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib settings 2>&1 | grep -E "^error" | head -3
```

Expected: `struct 'Settings' has no field named 'export'`.

- [ ] **Step 3: Give `ExportOptions` a `Default` and serde**

In `crates/app/src/batch_export.rs`, replace the `ExportOptions` derive line and add a `Default`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
```

Add the serde import at the top of the file:

```rust
use serde::{Deserialize, Serialize};
```

And after the struct:

```rust
impl Default for ExportOptions {
    /// ⚠ Needed for `#[serde(default)]` on `Settings` to fill this field when loading a
    /// config written before it existed. Without it, every existing user's settings file
    /// would fail to parse — and `load()` being infallible would turn that into *all*
    /// settings silently reverting to defaults, which is worse than an error.
    ///
    /// An empty destination means "never chosen", which `destination_ready` reports as not
    /// ready, so a fresh install still has to pick a folder.
    fn default() -> Self {
        Self {
            dest_dir: PathBuf::new(),
            quality: 92,
            max_edge: None,
        }
    }
}
```

- [ ] **Step 4: Add the field to `Settings`**

In `crates/app/src/settings.rs`, add the import and the field:

```rust
use crate::batch_export::ExportOptions;
```

```rust
pub struct Settings {
    /// Ask before deleting a preset. Cleared by "Don't ask again" in that dialog; restored
    /// from the Settings window.
    pub confirm_preset_delete: bool,
    /// The options the last export actually used. Not what was last *typed* — see
    /// `RustroomApp::start_export`.
    pub export: ExportOptions,
}
```

and in its `Default`:

```rust
        Self {
            confirm_preset_delete: true,
            export: ExportOptions::default(),
        }
```

- [ ] **Step 5: Run to verify they pass**

```bash
cargo test -p app --lib settings 2>&1 | tail -6
```

Expected: `test result: ok.`

- [ ] **Step 6: Wire it into the app**

In `crates/app/src/main.rs`, replace the hard-coded initialiser (around line 171):

```rust
            export_opts: settings.export.clone(),
```

⚠ `settings` is already bound above this (`let settings = settings_store.load();`), so no new load is needed.

Then in `start_export`, persist what was actually used. Find `fn start_export` and add at its top, before the worker is spawned:

```rust
        // ⚠ Persisted here rather than as the sliders move: these are the *last-used*
        // options, so tweaking the dialog and then cancelling must leave the remembered
        // settings alone. Same rule `Settings::apply_delete_confirm` follows for the
        // "don't ask again" tick, and it keeps the write count to one per export.
        if self.settings.export != self.export_opts {
            self.settings.export = self.export_opts.clone();
            self.save_settings();
        }
```

- [ ] **Step 7: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE "^(error|warning: )"
cargo test --all 2>&1 | grep -E "^test result"
git add -A
git commit -m "feat(app): remember the last-used export options across restarts"
```

Expected: clippy count `0`, all suites green.

---

## Task 3: `destination_ready`

**Files:**
- Modify: `crates/app/src/batch_export.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` in `crates/app/src/batch_export.rs`:

```rust
    #[test]
    fn destination_readiness_distinguishes_unset_missing_and_usable() {
        // ⚠ A pure function of the path rather than logic inside the egui closure, which is
        // what makes "Export is disabled when the folder is gone" testable at all rather
        // than only verifiable by clicking. Same extraction as
        // `Settings::apply_delete_confirm` and `view_region`'s maths.
        assert!(
            !destination_ready(Path::new("")),
            "an unset destination is not ready"
        );
        let d = TempDir::new("dest_ready");
        assert!(destination_ready(&d.0), "a real directory is ready");
        assert!(
            !destination_ready(&d.0.join("gone")),
            "a path that does not exist is not ready — the unplugged-drive case"
        );
        // A *file* is not a destination, and this is the case a plain `exists()` gets wrong.
        let file = d.0.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(!destination_ready(&file), "a file is not a directory");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib destination_readiness 2>&1 | grep -E "^error" | head -2
```

Expected: `cannot find function 'destination_ready' in this scope`.

- [ ] **Step 3: Implement**

In `crates/app/src/batch_export.rs`, after `dest_for`:

```rust
/// Whether a destination directory can be exported into.
///
/// Empty means never chosen; anything else means chosen once and possibly gone since — an
/// unplugged drive, a deleted folder, or a preset carried from another machine.
///
/// Checked **before** a run rather than per photo. `run` already survives an unwritable
/// destination without panicking, but failing two hundred photos one at a time and reporting
/// afterwards is a poor trade when the answer is knowable up front.
pub fn destination_ready(dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && dir.is_dir()
}
```

- [ ] **Step 4: Use it in the modal**

In `crates/app/src/main.rs`'s `export_modal`, replace the destination row and the readiness check.

The label gains a missing marker:

```rust
                let shown = if opts.dest_dir.as_os_str().is_empty() {
                    "(none chosen)".to_string()
                } else if opts.dest_dir.is_dir() {
                    opts.dest_dir.display().to_string()
                } else {
                    format!("{} (missing)", opts.dest_dir.display())
                };
                ui.label(shown);
```

and the button uses the shared rule:

```rust
                let ready = batch_export::destination_ready(&opts.dest_dir);
```

⚠ Add `destination_ready` to the `batch_export` import list at the top of `main.rs` if importing by name rather than qualifying.

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all && cargo test -p app --lib 2>&1 | grep -E "^test result"
git add -A
git commit -m "feat(app): disable Export when the destination is unset or missing"
```

---

## Task 4: `ExportPresetStore`

**Files:**
- Create: `crates/app/src/export_presets.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the module**

Create `crates/app/src/export_presets.rs`:

```rust
//! Named export settings, saved one JSON file per preset.
//!
//! ⚠ The shape mirrors `presets::PresetStore`, but **not for the same reason**. That store
//! chose one-file-per-preset primarily so a develop preset is a *shareable artifact*; an
//! export preset contains an absolute local path and is not shareable, so that argument does
//! not carry. What does: no read-modify-write race on save or delete, one corrupt file
//! cannot take the rest down, and it is one pattern to understand rather than two.
//!
//! ⚠ **No trash, no undo, no delete confirmation** — deliberately unlike develop presets. A
//! develop preset can encode an hour of grading; an export preset is three fields retyped in
//! ten seconds. The asymmetry is intentional, not an omission.

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
    /// Rooted at `dir`. Taken by injection so tests never touch the real config directory.
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
```

- [ ] **Step 2: Write the tests**

Append to `crates/app/src/export_presets.rs`:

```rust
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
            },
        }
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
```

- [ ] **Step 3: Declare the module**

In `crates/app/src/lib.rs`, after `pub mod edit_clipboard;`:

```rust
pub mod export_presets;
```

- [ ] **Step 4: Run**

```bash
cargo fmt --all && cargo test -p app --lib export_presets 2>&1 | tail -12
```

Expected: `test result: ok.` with 8 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(app): add ExportPresetStore, one JSON file per named export preset"
```

---

## Task 5: The preset row in the export modal

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the store and the cached list to `RustroomApp`**

Alongside `preset_store` / `preset_list` in the struct:

```rust
    export_preset_store: ExportPresetStore,
    /// Which preset the dialog currently shows, so Delete knows what to delete.
    ///
    /// ⚠ Tracked explicitly rather than inferred by matching the dialog's options against the
    /// list: two presets can hold identical options, and a Delete that guesses would remove
    /// the wrong one.
    selected_export_preset: Option<String>,
    /// Cached `export_preset_store.list()`, refreshed on save and delete rather than every
    /// frame — the same reason `preset_list` is cached.
    export_preset_list: Vec<ExportPreset>,
```

In `RustroomApp::new`, before the struct literal:

```rust
        let export_preset_store = ExportPresetStore::user_default();
        let export_preset_list = export_preset_store.list();
```

and in the literal:

```rust
            export_preset_store,
            export_preset_list,
            selected_export_preset: None,
```

Add the import at the top of `main.rs`:

```rust
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
```

- [ ] **Step 2: Add a modal variant for naming a preset**

In `enum Modal`, after `ExportSettings`:

```rust
    /// Naming a new export preset. Returns to `ExportSettings` when closed, so the export
    /// dialog does not vanish underneath the user.
    SaveExportPreset {
        name: String,
    },
```

- [ ] **Step 3: Add the row to `export_modal`**

Inside the `modal_dialog` closure in `export_modal`, immediately after the `ui.label(format!("Export {count} photos as JPEG."));` line:

```rust
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("export_preset_pick")
                    .selected_text("Preset…")
                    .show_ui(ui, |ui| {
                        for p in &presets {
                            let is_sel = selected.as_deref() == Some(p.name.as_str());
                            if ui.selectable_label(is_sel, &p.name).clicked() {
                                // Fills every field below, which is what keeps a preset's
                                // destination change visible rather than hidden.
                                picked = Some((p.name.clone(), p.options.clone()));
                            }
                        }
                    });
                if ui.button("Save as…").clicked() {
                    save_as = true;
                }
                let can_delete = !self.export_preset_list.is_empty();
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete"))
                    .clicked()
                {
                    delete_menu = true;
                }
            });
            ui.separator();
```

Declare the three locals beside the existing `pick_dir`, before the closure:

```rust
        let mut picked: Option<(String, ExportOptions)> = None;
        let mut save_as = false;
        let mut delete_menu = false;
        // ⚠ Snapshots taken *before* the closure. `self.export_preset_list` cannot be read
        // inside it while `opts` also borrows `self` — the same borrow shape that has already
        // cost a compile error in this file twice (see the `CLAUDE.md` note).
        let presets = self.export_preset_list.clone();
        let selected = self.selected_export_preset.clone();
```

⚠ `self.export_preset_list` is read inside a closure that also holds `&mut self` through `opts`. If that borrows, take a snapshot first: `let presets = self.export_preset_list.clone();` before the closure and iterate `&presets`. Two egui borrow shapes have already cost a compile error in this file — see the `CLAUDE.md` note on `self.session.as_ref().map(...)`.

After the closure, alongside the `pick_dir` handling:

```rust
        if let Some((name, o)) = picked {
            opts = o;
            self.selected_export_preset = Some(name);
        }
        if save_as {
            self.modal = Modal::SaveExportPreset {
                name: String::new(),
            };
        }
        if delete_menu {
            // No confirmation and no trash — see `export_presets`' module doc.
            if let Some(name) = self.selected_export_preset.take() {
                let _ = self.export_preset_store.delete(&name);
                self.export_preset_list = self.export_preset_store.list();
            }
        }
```

- [ ] **Step 4: Add the naming modal**

Next to the existing `save_preset_modal`, add:

```rust
    /// Naming a new export preset.
    fn save_export_preset_modal(&mut self, ctx: &egui::Context) {
        let Modal::SaveExportPreset { name } = &mut self.modal else {
            return;
        };
        let mut name = name.clone();
        let mut save = false;
        let mut close = false;
        let dismissed = modal_dialog(ctx, "save_export_preset", "Save export preset", |ui| {
            ui.text_edit_singleline(&mut name);
            ui.horizontal(|ui| {
                let ok = !name.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if save {
            let preset = ExportPreset {
                name: name.clone(),
                options: self.export_opts.clone(),
            };
            if let Err(e) = self.export_preset_store.save(&preset) {
                self.export_progress = format!("Could not save preset: {e}");
            }
            self.export_preset_list = self.export_preset_store.list();
            close = true;
        }
        if close || dismissed {
            // Back to the export dialog rather than to nothing, so it does not vanish.
            self.modal = Modal::ExportSettings;
        } else {
            self.modal = Modal::SaveExportPreset { name };
        }
    }
```

Dispatch it. ⚠ `main.rs` does **not** match on `self.modal` directly — it maps to a local
`Which` enum first and matches on *that*, because keeping the scrutinee borrowed across a
`&mut self` call is one of the two egui borrow shapes documented in `CLAUDE.md`. So there are
three edits, around `main.rs:1136-1166`:

```rust
        enum Which {
            None,
            Save,
            SaveExport,   // <- add
            ...
        }
        let which = match &self.modal {
            ...
            Modal::SaveExportPreset { .. } => Which::SaveExport,   // <- add
        };
        match which {
            ...
            Which::SaveExport => self.save_export_preset_modal(ctx),   // <- add
        }
```

- [ ] **Step 5: Verify the whole workspace**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all && cargo fmt --all --check && echo "FMT OK"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE "^(error|warning: )"
cargo test --all 2>&1 | grep -E "^test result"
```

Expected: fmt clean, clippy `0`, every suite green.

- [ ] **Step 6: Hunt for flakes**

```bash
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done; echo done
```

Expected: no output before `done`.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(app): named export presets in the export dialog"
```

---

## Task 6: Try it, then document

**Files:**
- Modify: `CLAUDE.md`

**⚠ One test the spec listed has no unit-test home**, and that is stated rather than quietly
dropped: `cancelling_the_export_dialog_leaves_the_remembered_options_alone`. The rule is
*structural* — the only write to `settings.export` lives in `start_export`, so Cancel cannot
persist anything because there is no code path that would. Testing it would mean driving
`RustroomApp`, which is the egui layer this crate deliberately does not unit-test. It is
verified by hand in Step 1 instead, and the structural guarantee is written into the comment
at the write site.

- [ ] **Step 1: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Tell the user what to check:

1. **Persistence**: export once, quit, relaunch — the destination, quality and long edge should come back. Then open the dialog, change the quality, **Cancel**, reopen: the change must *not* have stuck.
2. **A preset**: save one, change everything, pick it back — every field should visibly refill, destination included.
3. **The missing destination**: save a preset pointing at a folder, delete or rename that folder, pick the preset — the path should read `(missing)` and Export should be disabled.

- [ ] **Step 2: Update `CLAUDE.md`**

- Crate layout: add `safe_name` and `export_presets` to the `app` line.
- Add an "**Export presets done — M2.4 closed, Phase 2 complete**" section in the established style: the two-halves split; why persistence saves on *use* not on *edit*; the storage argument being weaker here than for develop presets and the reasons that do carry; the shared `safe_name` guard; **the deliberate no-trash asymmetry and its justification** (so a later session does not "fix" it); the pre-flight `destination_ready` check; and the caveats.
- **Milestones: M2.4 ✅ and Phase 2 ✅.** Note that the roadmap file in the Obsidian vault still needs its markers updated — it is outside the repo.
- Set the **next action**. Candidates, with the measured case for each: the **`user_qual` demosaic finding** (~93% of an 11 s decode is a default nobody chose; bilinear 405 ms vs AHD 10,148 ms, and every export pays it); a **second prefix cache checkpoint** (dehaze drags cost 87.6 ms against 13.7 ms for any other slider, and Straighten pays full price); **parallelising the RGBA8 pack** (~35% of a cached frame); or starting **Phase 3**.
- Add the spec and plan paths to the docs list.

- [ ] **Step 3: Final check and commit**

```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -5
git add -A
git commit -m "docs: record export presets; M2.4 closed and Phase 2 complete"
```

**Do not push.** The user pushes to `github.com/wladiKrin/rustroom` themselves.

---

## Definition of done

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` green
- [ ] 20 consecutive full-suite runs with no failures
- [ ] Every pre-existing `presets` test passes untouched, proving the `safe_name` extraction was behaviour-preserving
- [ ] A settings file written before this feature still loads, with the export options defaulted
- [ ] Persistence verified by hand: survives a restart, and **Cancel does not persist**
- [ ] A preset with a missing destination reads `(missing)` and leaves Export disabled
- [ ] `CLAUDE.md` marks **M2.4 ✅ and Phase 2 complete**, and names the next action
