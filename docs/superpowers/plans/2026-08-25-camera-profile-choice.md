# Choosing a camera profile (C4) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Discover `.dcp` profiles the user has installed, match them to the open photo's camera, and let them be chosen from the Develop panel — persisted per camera model.

**Architecture:** A `ProfileStore` scans a directory (path injected, like every other store here). `Settings` maps camera model → profile path. The three decode sites take the profile, and changing it re-decodes exactly as `view_demosaic` does. **No profiles are bundled.**

**Tech Stack:** Rust, existing `camera-profile` crate, egui.

**Spec:** `docs/superpowers/specs/2026-08-25-camera-profile-choice-design.md`

**Before anything:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
git checkout -b camera-profile-choice
```

---

## File Structure

| file | responsibility |
|---|---|
| `crates/camera-profile/src/store.rs` | **create** — scan a directory, match a model |
| `crates/camera-profile/src/lib.rs` | *modify* — declare the module |
| `crates/app/src/settings.rs` | *modify* — `camera_profiles: BTreeMap<String, PathBuf>` |
| `crates/app/src/main.rs` | *modify* — thread it into the three decode sites; re-decode on change |
| `crates/app/src/develop/panel/adjustments.rs` | *modify* — the dropdown |

---

### Task 1: discovery and matching

**Files:** create `crates/camera-profile/src/store.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_matches_its_own_profile() {
        assert!(matches_model("FUJIFILM X-T4", "X-T4"));
        assert!(matches_model("fujifilm x-t4", "X-T4"));
        assert!(matches_model("FUJIFILM X-T4", "x-t4"));
        assert!(matches_model("Canon EOS 5D Mark IV", "EOS 5D Mark IV"));
    }

    #[test]
    fn a_model_does_not_match_a_longer_one() {
        // ⚠⚠ The case that matters, and the reason plain containment is not enough:
        // "FUJIFILM X-T30" *contains* "X-T3", so a careless substring test makes nearly every
        // model a prefix of some other body and silently applies the wrong profile.
        assert!(!matches_model("FUJIFILM X-T30", "X-T3"));
        assert!(!matches_model("FUJIFILM X-T4", "X-T"));
        assert!(!matches_model("Canon EOS 5D Mark IV", "EOS 5D"));
    }

    #[test]
    fn an_unrelated_profile_does_not_match() {
        assert!(!matches_model("NIKON Z 8", "X-T4"));
        assert!(!matches_model("FUJIFILM X-T4", ""));
    }

    #[test]
    fn scanning_lists_profiles_and_skips_everything_else() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("notes.txt"), b"not a profile").unwrap();
        std::fs::write(dir.path().join("broken.dcp"), b"II\x00\x00").unwrap();
        std::fs::write(dir.path().join("good.dcp"), synthetic_profile("TEST CAM")).unwrap();

        let found = ProfileStore::new(dir.path().to_path_buf()).list();
        // ⚠ One bad file must not hide the rest — a directory of profiles is user-managed, and
        // a truncated download should cost one entry, not all of them.
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].name, "TEST CAM");
    }

    #[test]
    fn a_missing_directory_is_empty_rather_than_an_error() {
        // The common case on a fresh install: nobody has made the directory yet.
        let store = ProfileStore::new(std::path::PathBuf::from("/nonexistent/rustroom/profiles"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn matching_profiles_come_first_but_nothing_is_filtered_out() {
        // ⚠ The match **orders**, it never filters. An unusual model string must not make every
        // profile unreachable — the user can still pick by hand.
        let dir = TempDir::new();
        std::fs::write(dir.path().join("a.dcp"), synthetic_profile("NIKON Z 8")).unwrap();
        std::fs::write(dir.path().join("b.dcp"), synthetic_profile("FUJIFILM X-T4")).unwrap();
        let found = ProfileStore::new(dir.path().to_path_buf()).list_for("X-T4");
        assert_eq!(found.len(), 2, "nothing should be filtered out");
        assert_eq!(found[0].name, "FUJIFILM X-T4", "the match should be first");
    }
}
```

with a `TempDir` guard and a `synthetic_profile(name)` helper reusing the `synth` builder already
in `lib.rs`'s tests — ⚠ move `synth` and `matrix_bytes` into a `#[cfg(test)] pub(crate)` helper
module so both test modules share one builder rather than two copies drifting apart.

- [ ] **Step 2: Run to verify it fails**, then implement

```rust
/// Whether a profile's name names this camera model.
///
/// ⚠⚠ Containment **at word boundaries**, not plain containment. libraw reports `"X-T4"` while
/// the profile is `"FUJIFILM X-T4"`, so equality finds nothing — but plain containment matches
/// `"X-T3"` against `"FUJIFILM X-T30"`, which silently applies a different body's profile.
/// Requiring a non-alphanumeric character (or the end of the string) on both sides handles the
/// make prefix *and* multi-word models like `"EOS 5D Mark IV"`.
pub fn matches_model(profile_name: &str, model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    let (hay, needle) = (profile_name.to_uppercase(), model.to_uppercase());
    let bytes = hay.as_bytes();
    hay.match_indices(&needle).any(|(i, _)| {
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let end = i + needle.len();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        before_ok && after_ok
    })
}
```

plus `ProfileStore { dir }` with `new(dir)`, `list() -> Vec<ProfileEntry>` (sorted by name,
skipping unreadable and unparseable files) and `list_for(model)` (matches first, then the rest),
where `ProfileEntry { path, name }`.

- [ ] **Step 3: Sabotage**

| sabotage | must fail |
|---|---|
| plain `hay.contains(&needle)` | `a_model_does_not_match_a_longer_one` |
| compare without uppercasing | `a_model_matches_its_own_profile` |
| `list_for` filters instead of ordering | `matching_profiles_come_first_but_nothing_is_filtered_out` |
| `?` on the parse instead of skipping | `scanning_lists_profiles_and_skips_everything_else` |

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add -A && git commit -m "feat(camera-profile): discover and match installed profiles"
```

⚠ Check clippy's **exit code**, not grep output.

---

### Task 2: remember the choice per camera

**Files:** `crates/app/src/settings.rs`

- [ ] **Step 1: Tests**

```rust
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
        assert_eq!(back.camera_profiles.get("X-T4").map(|p| p.to_str().unwrap()), Some("/p/x-t4.dcp"));
    }
```

- [ ] **Step 2: Implement**

```rust
    /// Camera model → the `.dcp` profile chosen for it.
    ///
    /// ⚠ Keyed by **model**, not by photo: a profile is a calibration of the body. Per-photo is a
    /// recorded follow-up — it is what Lightroom does — but it would put a *decode* parameter into
    /// `Edits`, which would then have to travel through the sidecar, presets, copy/paste, sync and
    /// the buffer-cache key. See the C4 spec.
    ///
    /// ⚠ `BTreeMap` rather than `HashMap` so the settings file has a stable order and a diff of
    /// it is readable.
    pub camera_profiles: std::collections::BTreeMap<String, std::path::PathBuf>,
```

with `camera_profiles: BTreeMap::new()` in `Default`.

- [ ] **Step 3: Sabotage** — make `Default` insert an entry; `a_settings_file_from_before...` must fail.

- [ ] **Step 4: Commit**

---

### Task 3: use the profile when decoding

**Files:** `crates/app/src/main.rs`

- [ ] **Step 1: Thread it into all three decode sites**

`decode_for_edit`, `decode_full_res` and the export decode each gain an
`Option<PathBuf>` and set `camera_profile` on their `DecodeOptions`.

⚠ **`decode_for_edit` is currently a plain `fn(&Path)` handed to `DecodeWorker`.** It becomes a
closure capturing the profile — the same shape `full_res_worker(tier, ctx)` already uses for the
demosaic tier, and for the same reason: the worker's callers have no business knowing about it.

⚠ **Rebuilding the worker is what makes a change take effect**, and dropping the old one
disconnects its channel so an in-flight decode discards its own result — the abandon-don't-abort
trade already used at handover.

- [ ] **Step 2: `apply_camera_profile`**

Mirrors `apply_view_demosaic` exactly: persist, rebuild both decode workers, clear `full_pending`,
mark the buffer stale, and re-decode the open photo.

⚠ Clearing `full_pending` is load-bearing for the same reason it is there: that sequence number
belongs to the worker being dropped, and leaving it blocks `maybe_request_full_res` forever.

⚠ Also drop the photo's entry from `BufferCache` — otherwise reopening it serves the buffer
decoded with the *old* profile, which looks exactly like the setting not working.

- [ ] **Step 3: `cargo test --all`** — 39 goldens byte-identical; nothing sets a profile yet.

- [ ] **Step 4: Commit**

---

### Task 4: the dropdown

**Files:** `crates/app/src/develop/panel/adjustments.rs`

- [ ] **Step 1: Add it above white balance**, RAW only — a JPEG has no camera profile:

```rust
    // ⚠ RAW only: a JPEG carries no camera profile, and offering one would imply it did.
    if is_raw {
        ui.horizontal(|ui| {
            ui.label("Camera profile");
            egui::ComboBox::from_id_salt("camera_profile_pick")
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    // ⚠ "Built-in" is a real choice, not a fallback label: it selects C1's libraw
                    // matrix, the well-tested path and the oracle C2 and C3 are measured against.
                    // Someone who dislikes a profile must be able to get back to it.
                    ui.selectable_value(&mut chosen, None, "Built-in (libraw)");
                    for e in &entries {
                        ui.selectable_value(&mut chosen, Some(e.path.clone()), &e.name);
                    }
                });
        });
    }
```

⚠ The list is read once when the photo opens, not every frame — scanning a directory sixty times a
second is the `apply_view_demosaic` mistake in a different costume.

- [ ] **Step 2: Change guard**, then `apply_camera_profile`.

- [ ] **Step 3: Verify by hand**

```bash
mkdir -p ~/Library/Application\ Support/rustroom/profiles
cp "/Applications/RawTherapee.app/Contents/Resources/share/dcpprofiles/FUJIFILM X-T4.dcp" ~/Library/Application\ Support/rustroom/profiles/
cargo run --release -p app
```

Open an X-T4 RAW. The dropdown should offer **Built-in (libraw)** and **FUJIFILM X-T4**, with the
profile listed first. Selecting it should visibly change saturated colour; selecting Built-in
should change it back. ⚠ Switching photos must keep the choice — it is per camera.

- [ ] **Step 4: Update `CLAUDE.md`** — what landed, the ⚠ findings (not bundling and why; word-
boundary matching and the X-T3/X-T30 hazard; per-camera with per-photo recorded as a follow-up;
the buffer-cache eviction), the test count, the caveats. Update **▶ NEXT ACTION**: colour
management is complete; **M3.2** lens corrections, **M3.4** virtual copies.

- [ ] **Step 5: Commit.** ⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** no bundled profiles (throughout) ✓; discovery from the config directory with an
injected path (Task 1) ✓; word-boundary matching including the X-T30 case (Task 1) ✓; match orders
but never filters (Task 1) ✓; malformed file skipped (Task 1) ✓; per-camera persistence with
per-photo recorded (Task 2) ✓; re-decode on change (Task 3) ✓; dropdown with Built-in first
(Task 4) ✓; verified by hand (Task 4) ✓.

**Type consistency:** `ProfileStore::new/list/list_for`, `ProfileEntry { path, name }` and
`matches_model` are defined in Task 1 and used in Tasks 3–4. `Settings.camera_profiles` is defined
in Task 2 and used in Tasks 3–4.
