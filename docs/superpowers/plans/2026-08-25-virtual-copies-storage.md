# Virtual copies (M3.4), plan 1 of 2 — storage and identity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A photo's sidecar can hold several independent edit versions, every existing sidecar still loads with its edits intact, and identity is `(path, copy)` everywhere it needs to be — with no GUI change.

**Architecture:** A `PhotoId` newtype in the `app` lib; the sidecar becomes an envelope holding a list of `CopyEntry`, migrated from the legacy bare-`Edits` shape by **explicit key dispatch**, never `#[serde(untagged)]`. `HistoryStash` re-keys on `PhotoId`; `BufferCache` deliberately does not. Export filenames gain a copy suffix.

**Tech Stack:** Rust, `serde` / `serde_json`.

**Spec:** `docs/superpowers/specs/2026-08-25-virtual-copies-design.md`

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test and **confirm it compiled**; clippy clean checked by **exit code**.

⚠ **`cargo test --all | grep -c "test result: FAILED"` returns 0 when the build FAILS** — a failed build emits no test-result lines at all. This bit once already in M3.2. **Check the exit code**, always:

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

⚠ **`cd` persists between shell calls** — return to the repo root. ⚠ zsh does not word-split `$VAR`; use `${=VAR}`. ⚠ `ls` is aliased with colour; use `find` when capturing paths.

⚠⚠ **This plan touches the file that holds every user's edits. The migration is the one thing in M3.4 that can destroy data, and it can do so with a green test suite.** Task 2 is the task to slow down on.

---

### Task 1: `PhotoId`

**Files:**
- Create: `crates/app/src/photo_id.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/photo_id.rs`:

```rust
//! Which version of which file the app is looking at.
//!
//! ⚠ This replaces `PathBuf` as a photo's identity. A virtual copy is the same file with
//! different edits, so a path alone can no longer say which photo is meant.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_original_is_copy_zero() {
        let id = PhotoId::original(PathBuf::from("/x/a.RAF"));
        assert_eq!(id.copy, 0);
        assert!(id.is_original());
    }

    #[test]
    fn a_copy_is_not_the_original() {
        let id = PhotoId::new(PathBuf::from("/x/a.RAF"), 1);
        assert!(!id.is_original());
    }

    #[test]
    fn two_copies_of_one_file_are_different_photos() {
        // ⚠ The whole point of the type. If these compared equal, two copies would share a
        // history stash and a session, and editing one would appear to edit the other.
        let a = PhotoId::new(PathBuf::from("/x/a.RAF"), 1);
        let b = PhotoId::new(PathBuf::from("/x/a.RAF"), 2);
        assert_ne!(a, b);
    }

    #[test]
    fn the_suffix_is_empty_for_the_original_and_numbered_otherwise() {
        // ⚠ **One number used everywhere**: this same value is the identity, the label and the
        // export filename suffix. A display number offset from the identity number is two facts
        // that can disagree — the failure `render_max_dim` and `fit_image_size` were both fixed
        // for. Deliberately *not* Lightroom's labelling, where the original is "Copy 1".
        assert_eq!(PhotoId::original(PathBuf::from("/x/a.RAF")).suffix(), "");
        assert_eq!(PhotoId::new(PathBuf::from("/x/a.RAF"), 1).suffix(), "_1");
        assert_eq!(PhotoId::new(PathBuf::from("/x/a.RAF"), 12).suffix(), "_12");
    }

    #[test]
    fn the_label_names_the_file_and_the_copy() {
        assert_eq!(
            PhotoId::original(PathBuf::from("/x/DSCF6223.RAF")).label(),
            "DSCF6223.RAF"
        );
        assert_eq!(
            PhotoId::new(PathBuf::from("/x/DSCF6223.RAF"), 1).label(),
            "DSCF6223.RAF / 1"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd ~/Projects/rustroom
cargo test -p app --lib photo_id 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type PhotoId in this scope`.

- [ ] **Step 3: Implement**

Insert above the `#[cfg(test)]` module:

```rust
use std::path::{Path, PathBuf};

/// One version of one file: the file's path plus which copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhotoId {
    pub path: PathBuf,
    /// `0` is the original. Copies are numbered from 1 and **never reused**.
    ///
    /// ⚠ This one number is the identity, the number shown in the UI, and the export filename
    /// suffix. Keeping them the same value means they cannot drift apart.
    pub copy: u32,
}

impl PhotoId {
    pub fn new(path: PathBuf, copy: u32) -> Self {
        Self { path, copy }
    }

    /// The file itself, as opposed to any of its virtual copies.
    pub fn original(path: PathBuf) -> Self {
        Self { path, copy: 0 }
    }

    pub fn is_original(&self) -> bool {
        self.copy == 0
    }

    /// What an export filename appends: nothing for the original, `_N` for copy N.
    ///
    /// ⚠ Underscore, not hyphen. `batch_export::dest_for` already uses `-N` for a *different*
    /// meaning — "the Nth file wanting this name in this folder", assigned at export time. This
    /// one is a property of the photo and stable forever. One separator per meaning.
    pub fn suffix(&self) -> String {
        if self.is_original() {
            String::new()
        } else {
            format!("_{}", self.copy)
        }
    }

    /// What the UI shows: the file name, plus ` / N` for a copy.
    pub fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unnamed)");
        if self.is_original() {
            name.to_string()
        } else {
            format!("{name} / {}", self.copy)
        }
    }

    /// Borrow the path, for the many APIs that still key on the file rather than the version —
    /// `BufferCache` above all, where copies deliberately share one decoded buffer.
    pub fn file(&self) -> &Path {
        &self.path
    }
}
```

- [ ] **Step 4: Declare the module**

In `crates/app/src/lib.rs`, add beside the other `pub mod` lines, keeping alphabetical order:

```rust
pub mod photo_id;
```

- [ ] **Step 5: Run**

```bash
cargo test -p app --lib photo_id 2>&1 | grep -E "test result:|FAILED"
```

Expected: five PASS.

- [ ] **Step 6: Sabotage — drop `copy` from equality**

Change the derive to `#[derive(Debug, Clone, Eq, Hash, PartialOrd, Ord)]` and hand-write:

```rust
impl PartialEq for PhotoId {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
```

Confirm it **compiles**, then run.

Expected: `two_copies_of_one_file_are_different_photos` FAILS. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/photo_id.rs crates/app/src/lib.rs
git commit -m "feat(app): PhotoId — a file plus which copy of it"
```

---

### Task 2: The sidecar envelope and its migration

⚠⚠ **The task to slow down on.** Getting this wrong silently discards every edit anyone has made, and the natural implementation is the one that does exactly that.

**Files:**
- Modify: `crates/app/src/sidecar.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/sidecar.rs`'s test module:

```rust
    #[test]
    fn a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact() {
        // ⚠⚠ THE test of this feature. Every sidecar written before today is a bare `Edits`
        // object. The obvious implementation — an untagged enum of { Envelope, LegacyEdits } —
        // reads *every one of them as an empty copy list*, because `Edits` carries a
        // container-level `#[serde(default)]` and untagged matching accepts any object. The
        // suite stays green and every user's edits are gone.
        //
        // ⚠ So this asserts the **payload**, not the variant. M3.1b's
        // `an_existing_linear_sidecar_still_loads` asserted only the variant and passed while
        // reading nothing at all.
        let dir = TempDir::new("legacy_load");
        let photo = dir.0.join("a.RAF");
        std::fs::write(
            path(&photo),
            r#"{"exposure":1.25,"contrast":40.0,"vibrance":-15.0}"#,
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.copies.len(), 1, "a legacy sidecar is exactly one copy");
        assert_eq!(s.copies[0].copy, 0);
        assert_eq!(s.copies[0].name, None);
        assert_eq!(s.copies[0].edits.exposure, 1.25, "exposure was lost");
        assert_eq!(s.copies[0].edits.contrast, 40.0, "contrast was lost");
        assert_eq!(s.copies[0].edits.vibrance, -15.0, "vibrance was lost");
    }

    #[test]
    fn an_envelope_round_trips_numbers_names_and_edits() {
        let dir = TempDir::new("envelope_round_trip");
        let photo = dir.0.join("a.RAF");
        let want = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    name: None,
                    edits: Edits {
                        exposure: 0.5,
                        ..Edits::default()
                    },
                },
                CopyEntry {
                    copy: 3,
                    name: Some("B&W".to_string()),
                    edits: Edits {
                        saturation: -100.0,
                        ..Edits::default()
                    },
                },
            ],
        };
        save_all(&photo, &want).unwrap();
        assert_eq!(load_all(&photo), want);
    }

    #[test]
    fn an_absent_sidecar_is_an_empty_envelope() {
        let dir = TempDir::new("absent");
        assert_eq!(load_all(&dir.0.join("nope.RAF")), Sidecar::default());
    }

    #[test]
    fn a_malformed_sidecar_is_an_empty_envelope_rather_than_a_panic() {
        // The existing posture: a hand-edited file with a typo reads as "no edits", it does not
        // take anything down.
        let dir = TempDir::new("malformed");
        let photo = dir.0.join("a.RAF");
        std::fs::write(path(&photo), "{ not json at all").unwrap();
        assert_eq!(load_all(&photo), Sidecar::default());
    }

    #[test]
    fn copy_numbers_are_never_reused() {
        // ⚠ Renumbering would silently change a copy's export filename and invalidate its
        // history-stash key — the undo stack would attach to a different version than the one
        // on screen.
        let mut s = Sidecar::default();
        assert_eq!(s.next_copy(), 1, "the first copy is 1, not 0");
        s.copies.push(CopyEntry {
            copy: 0,
            name: None,
            edits: Edits::default(),
        });
        s.copies.push(CopyEntry {
            copy: 1,
            name: None,
            edits: Edits::default(),
        });
        s.copies.push(CopyEntry {
            copy: 2,
            name: None,
            edits: Edits::default(),
        });
        s.copies.retain(|c| c.copy != 1);
        assert_eq!(s.next_copy(), 3, "2 is gone but must not come back");
    }

    #[test]
    fn a_sidecar_holding_only_a_neutral_original_is_neutral() {
        // ⚠ Preserves the existing "Reset all deletes the sidecar" behaviour, which was itself a
        // bug fix: skipping the write left a stale file to be loaded straight back.
        let mut s = Sidecar::default();
        assert!(s.is_neutral(), "empty");
        s.copies.push(CopyEntry {
            copy: 0,
            name: None,
            edits: Edits::default(),
        });
        assert!(s.is_neutral(), "a neutral, unnamed original only");

        let named = Sidecar {
            copies: vec![CopyEntry {
                copy: 0,
                name: Some("Hero".to_string()),
                edits: Edits::default(),
            }],
        };
        assert!(!named.is_neutral(), "a name is worth keeping the file for");

        let two = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    name: None,
                    edits: Edits::default(),
                },
                CopyEntry {
                    copy: 1,
                    name: None,
                    edits: Edits::default(),
                },
            ],
        };
        assert!(!two.is_neutral(), "a copy is worth keeping the file for");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib sidecar 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type Sidecar`.

- [ ] **Step 3: Implement the types and the migration**

In `crates/app/src/sidecar.rs`, add after the `use` lines:

```rust
use serde::{Deserialize, Serialize};

/// One version of a photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyEntry {
    /// `0` is the original; copies are numbered from 1 and never reused.
    pub copy: u32,
    /// The user's label for this copy.
    ///
    /// ⚠ Deliberately **not** an `Edits` field. In `Edits` it would need an edit group and would
    /// then travel with a paste, a preset or a sync — pasting a B&W recipe onto another photo
    /// would rename it "B&W" — and "Reset adjustments" would wipe it.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub edits: Edits,
}

/// Everything stored for one file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sidecar {
    pub copies: Vec<CopyEntry>,
}

impl Sidecar {
    pub fn get(&self, copy: u32) -> Option<&CopyEntry> {
        self.copies.iter().find(|c| c.copy == copy)
    }

    /// The number a new copy should take: one past the highest ever used here.
    ///
    /// ⚠ `max + 1`, never "first free". Reusing a number would point a new copy at an old
    /// copy's export filename and history stash.
    pub fn next_copy(&self) -> u32 {
        self.copies.iter().map(|c| c.copy).max().map_or(1, |m| m + 1)
    }

    /// Replace (or add) one copy's edits, leaving its name alone.
    pub fn set_edits(&mut self, copy: u32, edits: Edits) {
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => c.edits = edits,
            None => self.copies.push(CopyEntry {
                copy,
                name: None,
                edits,
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Whether this sidecar is worth writing at all.
    ///
    /// ⚠ Only true for "nothing but a neutral, unnamed original". A name or a second copy is
    /// worth a file even with neutral edits, or making a copy and then resetting it would delete
    /// the copy.
    pub fn is_neutral(&self) -> bool {
        match self.copies.as_slice() {
            [] => true,
            [only] => only.copy == 0 && only.name.is_none() && only.edits == Edits::default(),
            _ => false,
        }
    }
}

/// Every version stored for `photo`.
///
/// ⚠⚠ **The dispatch is explicit, and must never become `#[serde(untagged)]`.** `Edits` carries
/// a container-level `#[serde(default)]`, so an untagged enum would accept *any* JSON object as
/// an envelope and read every legacy sidecar as an empty copy list — discarding every edit ever
/// made, with a green test suite. That exact mechanism already cost this project a test that
/// passed while reading nothing (`an_existing_linear_sidecar_still_loads`, M3.1b).
pub fn load_all(photo: &Path) -> Sidecar {
    let Ok(text) = std::fs::read_to_string(path(photo)) else {
        return Sidecar::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Sidecar::default();
    };
    if value.get("copies").is_some() {
        serde_json::from_value(value).unwrap_or_default()
    } else {
        // Legacy: the whole file is one bare `Edits`, which is copy 0.
        match serde_json::from_value::<Edits>(value) {
            Ok(edits) => Sidecar {
                copies: vec![CopyEntry {
                    copy: 0,
                    name: None,
                    edits,
                }],
            },
            Err(_) => Sidecar::default(),
        }
    }
}

/// Write every version, unconditionally.
pub fn save_all(photo: &Path, s: &Sidecar) -> Result<(), EditLoadError> {
    let text = serde_json::to_string_pretty(s)?;
    std::fs::write(path(photo), text)?;
    Ok(())
}
```

⚠ `serde_json` must be a dependency of `app`; it already is (the presets and settings stores use it). If `EditLoadError` does not convert from `serde_json::Error` and `std::io::Error`, use whatever conversions `Edits::to_json_file` already relies on rather than inventing new ones.

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib sidecar 2>&1 | grep -E "test result:|FAILED" | head -3
```

Expected: all PASS.

- [ ] **Step 5: Sabotage — the trap itself**

Replace `load_all`'s dispatch with the untagged version the design warns about:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum Either {
    Envelope(Sidecar),
    Legacy(Edits),
}

pub fn load_all(photo: &Path) -> Sidecar {
    let Ok(text) = std::fs::read_to_string(path(photo)) else {
        return Sidecar::default();
    };
    match serde_json::from_str::<Either>(&text) {
        Ok(Either::Envelope(s)) => s,
        Ok(Either::Legacy(edits)) => Sidecar {
            copies: vec![CopyEntry { copy: 0, name: None, edits }],
        },
        Err(_) => Sidecar::default(),
    }
}
```

Confirm it **compiles**, then run.

Expected: `a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact` FAILS — the legacy file matches `Envelope` first, `copies` defaults to empty, and `copies.len()` is 0 rather than 1. **This is the data-loss bug, demonstrated.** Restore.

⚠ If it does *not* fail, stop and find out why before continuing — it means the guard is not guarding.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(app): the sidecar holds several edit versions"
```

---

### Task 3: Per-copy load and persist

**Files:**
- Modify: `crates/app/src/sidecar.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn persisting_one_copy_leaves_the_others_alone() {
        // ⚠ Read-modify-write, not overwrite. Two copies of one file live in one file, so
        // saving one must not drop the other.
        let dir = TempDir::new("persist_one");
        let photo = dir.0.join("a.RAF");
        save_all(
            &photo,
            &Sidecar {
                copies: vec![
                    CopyEntry {
                        copy: 0,
                        name: None,
                        edits: Edits {
                            exposure: 1.0,
                            ..Edits::default()
                        },
                    },
                    CopyEntry {
                        copy: 1,
                        name: Some("B&W".to_string()),
                        edits: Edits {
                            saturation: -100.0,
                            ..Edits::default()
                        },
                    },
                ],
            },
        )
        .unwrap();

        persist_copy(
            &photo,
            0,
            &Edits {
                exposure: 2.0,
                ..Edits::default()
            },
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.get(0).unwrap().edits.exposure, 2.0, "copy 0 not updated");
        assert_eq!(s.get(1).unwrap().edits.saturation, -100.0, "copy 1 lost");
        assert_eq!(
            s.get(1).unwrap().name.as_deref(),
            Some("B&W"),
            "copy 1's name lost"
        );
    }

    #[test]
    fn persisting_a_copy_never_touches_its_name() {
        let dir = TempDir::new("persist_name");
        let photo = dir.0.join("a.RAF");
        save_all(
            &photo,
            &Sidecar {
                copies: vec![CopyEntry {
                    copy: 1,
                    name: Some("Punchy".to_string()),
                    edits: Edits::default(),
                }],
            },
        )
        .unwrap();
        persist_copy(
            &photo,
            1,
            &Edits {
                contrast: 30.0,
                ..Edits::default()
            },
        )
        .unwrap();
        let s = load_all(&photo);
        assert_eq!(s.get(1).unwrap().name.as_deref(), Some("Punchy"));
        assert_eq!(s.get(1).unwrap().edits.contrast, 30.0);
    }

    #[test]
    fn resetting_the_only_copy_to_neutral_deletes_the_sidecar() {
        // The behaviour `persist` already had, preserved through the envelope: skipping the
        // write instead would leave a stale file to be loaded straight back — which is exactly
        // what "Reset all, switch away, switch back" used to do.
        let dir = TempDir::new("neutral_delete");
        let photo = dir.0.join("a.RAF");
        persist_copy(
            &photo,
            0,
            &Edits {
                exposure: 1.0,
                ..Edits::default()
            },
        )
        .unwrap();
        assert!(path(&photo).exists());
        persist_copy(&photo, 0, &Edits::default()).unwrap();
        assert!(!path(&photo).exists(), "a neutral original leaves no file");
    }

    #[test]
    fn resetting_one_of_two_copies_keeps_the_sidecar() {
        // ⚠ The case the naive rule gets wrong: "edits are neutral, so delete the file" would
        // throw away the *other* copy.
        let dir = TempDir::new("neutral_two");
        let photo = dir.0.join("a.RAF");
        save_all(
            &photo,
            &Sidecar {
                copies: vec![
                    CopyEntry {
                        copy: 0,
                        name: None,
                        edits: Edits {
                            exposure: 1.0,
                            ..Edits::default()
                        },
                    },
                    CopyEntry {
                        copy: 1,
                        name: None,
                        edits: Edits {
                            exposure: 2.0,
                            ..Edits::default()
                        },
                    },
                ],
            },
        )
        .unwrap();
        persist_copy(&photo, 0, &Edits::default()).unwrap();
        assert!(path(&photo).exists(), "copy 1 must survive");
        assert_eq!(load_all(&photo).get(1).unwrap().edits.exposure, 2.0);
    }

    #[test]
    fn loading_one_copy_returns_only_that_copys_edits() {
        let dir = TempDir::new("load_one");
        let photo = dir.0.join("a.RAF");
        save_all(
            &photo,
            &Sidecar {
                copies: vec![
                    CopyEntry {
                        copy: 0,
                        name: None,
                        edits: Edits {
                            exposure: 1.0,
                            ..Edits::default()
                        },
                    },
                    CopyEntry {
                        copy: 1,
                        name: None,
                        edits: Edits {
                            exposure: 2.0,
                            ..Edits::default()
                        },
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(load_copy(&photo, 0).unwrap().exposure, 1.0);
        assert_eq!(load_copy(&photo, 1).unwrap().exposure, 2.0);
        assert!(load_copy(&photo, 9).is_none(), "an absent copy is None");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib sidecar 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find function persist_copy`.

- [ ] **Step 3: Implement**

Add to `crates/app/src/sidecar.rs`:

```rust
/// One copy's stored edits, or `None` when that copy has none.
pub fn load_copy(photo: &Path, copy: u32) -> Option<Edits> {
    load_all(photo).get(copy).map(|c| c.edits.clone())
}

/// Persist one copy's edits, leaving every other copy — and this copy's name — untouched.
///
/// ⚠ Read-modify-write, because several versions share one file. The sidecar is removed only
/// when nothing is left worth storing; see [`Sidecar::is_neutral`].
pub fn persist_copy(photo: &Path, copy: u32, edits: &Edits) -> Result<(), EditLoadError> {
    let mut s = load_all(photo);
    s.set_edits(copy, edits.clone());
    persist_all(photo, &s)
}

/// Write every version, or delete the file when there is nothing worth keeping.
pub fn persist_all(photo: &Path, s: &Sidecar) -> Result<(), EditLoadError> {
    if s.is_neutral() {
        return remove(photo).map_err(EditLoadError::from);
    }
    save_all(photo, s)
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib sidecar 2>&1 | grep -E "test result:|FAILED" | head -3
```

Expected: all PASS.

- [ ] **Step 5: Sabotage — make `persist_copy` overwrite instead of merge**

Replace its body with:

```rust
    let mut s = Sidecar::default();
    s.set_edits(copy, edits.clone());
    persist_all(photo, &s)
```

Confirm it compiles, then run.

Expected: `persisting_one_copy_leaves_the_others_alone`, `persisting_a_copy_never_touches_its_name` and `resetting_one_of_two_copies_keeps_the_sidecar` all FAIL. Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(app): load and persist one copy at a time"
```

---

### Task 4: `HistoryStash` **and `ActionLog`** key on `PhotoId`

⚠⚠ **These two must change together, and this project has already learned why.** CLAUDE.md, on the
per-photo edit history: *"The storage was the easy half; the log was the feature. A per-path history
alone does nothing, because `ActionKind::PhotoEdit` was **anonymous** — it meant 'pop whatever
history is current'."* Re-keying the stash per copy while `ActionKind::PhotoEdit` still carries a
bare `PathBuf` would make Ctrl+Z on the original pop a copy's stack. One mechanism, one commit.

**Files:**
- Modify: `crates/app/src/edit_history.rs`
- Modify: `crates/app/src/action_log.rs` (`ActionKind::PhotoEdit`, `forget_edits_of`, `reachable`)
- Modify: `crates/app/src/main.rs` (call sites)

- [ ] **Step 1: Write the failing test**

Add to `crates/app/src/edit_history.rs`'s test module:

```rust
    #[test]
    fn two_copies_of_one_file_keep_separate_stacks() {
        // ⚠ The reason the stash re-keys. Keyed by path, editing a copy would hand you the
        // original's undo stack, and `take_matching`'s equality check would then silently
        // discard whichever stack lost.
        use crate::photo_id::PhotoId;
        let mut s = HistoryStash::default();
        let path = std::path::PathBuf::from("/x/a.RAF");

        let mut h0 = History::new(Edits::default());
        h0.push(Edits {
            exposure: 1.0,
            ..Edits::default()
        });
        let mut h1 = History::new(Edits::default());
        h1.push(Edits {
            exposure: 2.0,
            ..Edits::default()
        });

        s.stash(PhotoId::original(path.clone()), h0);
        s.stash(PhotoId::new(path.clone(), 1), h1);
        assert_eq!(s.len(), 2, "one stack per copy, not per file");

        let got = s
            .take_matching(
                &PhotoId::new(path, 1),
                &Edits {
                    exposure: 2.0,
                    ..Edits::default()
                },
            )
            .expect("copy 1's stack");
        assert_eq!(got.get().exposure, 2.0);
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib two_copies_of_one_file_keep 2>&1 | grep -E "^error" | head -3
```

Expected: a type mismatch — `stash` expects `PathBuf`.

- [ ] **Step 3: Re-key the stash**

In `crates/app/src/edit_history.rs`, change the map and every signature from `PathBuf`/`&Path` to `PhotoId`/`&PhotoId`:

```rust
use crate::photo_id::PhotoId;

pub struct HistoryStash {
    /// ⚠ Keyed by [`PhotoId`], not by path: two copies of one file are two photos with two
    /// undo stacks. `BufferCache` deliberately keys the other way — see its doc comment.
    map: HashMap<PhotoId, History<Edits>>,
}
```

and correspondingly `stash(&mut self, id: PhotoId, ..)` and
`take_matching(&mut self, id: &PhotoId, on_disk: &Edits)`.

Then in `crates/app/src/action_log.rs`, change `ActionKind::PhotoEdit(PathBuf)` to
`ActionKind::PhotoEdit(PhotoId)`, and `ActionLog::forget_edits_of(&mut self, path: &Path)` to take
`&PhotoId`. ⚠ `reachable()` compares the entry's photo against the open one — it now compares
`PhotoId`s, which is what scopes undo to *this copy* rather than to the file.

⚠ Add this test to `action_log.rs`:

```rust
    #[test]
    fn undo_on_the_original_cannot_reach_a_copys_edit() {
        // ⚠ The half that the stash alone does not give you. With `PhotoEdit` carrying a bare
        // path, an edit made on copy 1 would be reachable from the original and Ctrl+Z there
        // would pop the wrong stack.
        use crate::photo_id::PhotoId;
        let path = std::path::PathBuf::from("/a.RAF");
        let original = PhotoId::original(path.clone());
        let copy = PhotoId::new(path, 1);

        let mut log = ActionLog::default();
        log.push(ActionKind::PhotoEdit(copy.clone()));
        assert!(log.undo(&original).is_none(), "the original must not reach it");
        assert!(log.undo(&copy).is_some(), "its own copy must");
    }
```

⚠ `ActionLog::undo`/`push` may take different argument shapes — read the existing tests in that
file and match them rather than inventing a signature; the assertion above is what matters.

- [ ] **Step 4: Fix the call sites**

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 4 | head -20
```

Every site in `main.rs` that passes a path now passes `PhotoId::original(path.clone())` — plan 2 replaces those with the real copy. Add a comment at each:

```rust
    // ⚠ `PhotoId::original` until the Library knows about copies — plan 2 of M3.4.
```

- [ ] **Step 5: Run**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

Expected: `exit: 0`.

- [ ] **Step 6: Sabotage — key the stash on the path again**

Change the map key back to `PhotoId`'s path by making `stash` use `PhotoId::original(id.path)`. Confirm it compiles, then run.

Expected: `two_copies_of_one_file_keep_separate_stacks` FAILS with `len()` of 1. Restore.

- [ ] **Step 6b: Sabotage — make `reachable` compare only the path**

In `action_log.rs`, change the `PhotoEdit` arm of `reachable` to compare `id.path == open.path`
instead of `id == open`. Confirm it compiles, then run.

Expected: `undo_on_the_original_cannot_reach_a_copys_edit` FAILS. Restore.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(app): undo is scoped to a copy, not to a file"
```

---

### Task 5: `BufferCache` stays keyed by path — asserted, not assumed

**Files:**
- Modify: `crates/app/src/buffer_cache.rs`

⚠ This task adds **no production code**. The storage claim in the roadmap — "multiple edit versions per file at no storage cost" — is delivered entirely by a decision *not* to change something, and an undefended decision is one somebody changes.

- [ ] **Step 1: Write the test**

Add to `crates/app/src/buffer_cache.rs`'s test module:

```rust
    #[test]
    fn copies_of_one_file_share_one_decoded_buffer() {
        // ⚠⚠ This is the roadmap's "at no storage cost", and it is delivered by *not* keying
        // this cache on `PhotoId`. A decode is 78–313 MB; two copies of one photo must never
        // hold two of them.
        //
        // The test is written against `PhotoId` on purpose: it is the type a future change would
        // reach for, and this asserts that reaching for it changes nothing here.
        use crate::photo_id::PhotoId;
        let mut cache = BufferCache::new(3);
        let path = std::path::PathBuf::from("/x/a.RAF");
        let original = PhotoId::original(path.clone());
        let copy = PhotoId::new(path.clone(), 1);

        cache.insert(path.clone(), decoded("a"));
        assert!(
            cache.get(original.file()).is_some(),
            "the original must hit"
        );
        assert!(
            cache.get(copy.file()).is_some(),
            "and so must its copy — same file, same decode"
        );
        assert_eq!(cache.len(), 1, "one buffer, not two");
    }
```

⚠ `decoded(label)` and `BufferCache::new(cap)` are the existing helpers in that file — `new`, not `default()`. `len()` is already public.

- [ ] **Step 2: Run**

```bash
cargo test -p app --lib copies_of_one_file_share 2>&1 | grep -E "test result:|^error" | head -3
```

Expected: PASS immediately — nothing needed changing, which is the point.

- [ ] **Step 3: Sabotage — key the cache on `PhotoId`**

Temporarily change `BufferCache`'s map key to `PhotoId` and `insert`/`get` accordingly, with the call site passing `PhotoId::original(path)`. Confirm it compiles, then run.

Expected: the test FAILS — `cache.get(copy.file())` no longer type-checks, or misses. **That failure is the documentation**: it shows exactly what a future change would break. Restore.

⚠ If the sabotage merely fails to compile, that is an acceptable outcome here — record it in the commit message rather than forcing a runtime failure.

- [ ] **Step 4: Document the decision on the type**

Add to `BufferCache`'s doc comment:

```rust
/// ⚠ **Keyed by path, never by `PhotoId`, and that is load-bearing.** Virtual copies of one file
/// share one decoded buffer — which *is* M3.4's "multiple edit versions at no storage cost". A
/// decode is 78–313 MB, so keying this on the copy number would multiply the largest allocation
/// in the app by the number of versions. `copies_of_one_file_share_one_decoded_buffer` guards it.
```

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/buffer_cache.rs
git commit -m "test(app): copies share one decoded buffer"
```

---

### Task 6: Export filenames carry the copy number

**Files:**
- Modify: `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/batch_export.rs`'s test module:

```rust
    #[test]
    fn a_copy_exports_with_an_underscore_suffix() {
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/DSCF6223.RAF"), 0, 0, ExportFormat::Jpeg),
            dir.join("DSCF6223.jpg"),
            "the original keeps the plain stem"
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/DSCF6223.RAF"), 2, 0, ExportFormat::Jpeg),
            dir.join("DSCF6223_2.jpg")
        );
    }

    #[test]
    fn the_two_numbering_schemes_compose_rather_than_collide() {
        // ⚠ `_N` is **identity** — which virtual copy, stable forever. `-N` is
        // **de-duplication** — the Nth file wanting this name in this folder, assigned at export
        // time. They mean different things, so they get different separators and both can appear.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 2, 1, ExportFormat::Jpeg),
            dir.join("a_2-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, 3, ExportFormat::Jpeg),
            dir.join("a-3.jpg"),
            "an original that collides is unchanged from before this feature"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib dest_for 2>&1 | grep -E "^error" | head -3
```

Expected: `this function takes 4 arguments but 5 were supplied`.

- [ ] **Step 3: Implement**

Change `dest_for` in `crates/app/src/batch_export.rs`:

```rust
/// Where one export lands.
///
/// ⚠ **Two numbers, two meanings, two separators.** `copy` is which virtual copy this is — a
/// property of the photo, stable forever, rendered `_N`. `dedup` is how many files already
/// wanted this name in this folder — a property of the destination, assigned at export time,
/// rendered `-N`. Merging them into one scheme would make `a-2.jpg` ambiguous between "copy 2"
/// and "the third file called a", and would silently shift a copy's number when its plain name
/// happened to be taken.
pub fn dest_for(
    dir: &Path,
    source: &Path,
    copy: u32,
    dedup: usize,
    format: ExportFormat,
) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let ext = format.extension();
    let copy_suffix = if copy == 0 {
        String::new()
    } else {
        format!("_{copy}")
    };
    let name = if dedup == 0 {
        format!("{stem}{copy_suffix}.{ext}")
    } else {
        format!("{stem}{copy_suffix}-{dedup}.{ext}")
    };
    dir.join(name)
}
```

- [ ] **Step 4: Fix the existing call sites**

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 3 | head -20
```

The two production sites (around lines 283 and 327) pass `0` for `copy` for now — plan 2 supplies the real number when jobs carry a `PhotoId`. Add at the first:

```rust
        // ⚠ Copy 0 until export jobs carry a `PhotoId` — plan 2 of M3.4.
```

Existing tests that call `dest_for(&dir, path, N, format)` gain a `0` in the new `copy` position; ⚠ **read each one** — their `N` is a *dedup* index, so it stays in the fourth position, not the third.

- [ ] **Step 5: Run**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
grep -E "test result: FAILED" /tmp/t.log | head -3
```

Expected: `exit: 0`.

- [ ] **Step 6: Sabotage — use one separator for both**

Change `copy_suffix` to `format!("-{copy}")`. Confirm it compiles, then run.

Expected: `a_copy_exports_with_an_underscore_suffix` and `the_two_numbering_schemes_compose_rather_than_collide` FAIL. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/batch_export.rs
git commit -m "feat(app): export filenames carry the copy number"
```

---

### Task 7: Measure the folder-open cost

⚠ The design commits to measuring this rather than assuming it. Opening a folder must now read every sidecar to know how many copies each file has.

**Files:**
- Create: `crates/app/examples/sidecar_scan.rs`

- [ ] **Step 1: Write the probe**

```rust
//! How long does reading every sidecar in a folder take?
//!
//! ⚠ M3.4 makes folder-open read one sidecar per file, where it previously read none. The design
//! committed to measuring that rather than assuming it was free.
//!
//! `cargo run --release -p app --example sidecar_scan <folder>`
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: sidecar_scan <dir>"));
    let entries = catalog::scan_folder(&dir).expect("scan");

    let t = std::time::Instant::now();
    let mut copies = 0usize;
    for e in &entries {
        copies += rustroom_app::sidecar::load_all(&e.path).copies.len();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;

    println!(
        "{} files, {copies} stored copies, {ms:.1} ms ({:.3} ms/file)",
        entries.len(),
        ms / entries.len().max(1) as f64
    );
}
```

- [ ] **Step 2: Run it on the real folder**

```bash
cd ~/Projects/rustroom
cargo run --release -q -p app --example sidecar_scan -- "$HOME/Desktop/fuji" 2>&1 | tail -3
```

Record the number. ⚠ **If it exceeds ~100 ms**, do not proceed as designed — the fallback stated in the spec is moving the read onto the loader thread, and that changes plan 2's shape. Under ~100 ms, note it and carry on.

- [ ] **Step 3: Commit**

```bash
git add crates/app/examples/sidecar_scan.rs
git commit -m "test(app): measure the cost of reading every sidecar"
```

---

## Definition of done for plan 1

- `PhotoId` exists and is the key for `HistoryStash`.
- The sidecar holds several versions; **every legacy sidecar loads with its edits intact**, proven by a test that asserts the payload and by watching the untagged implementation fail it.
- `persist_copy` is read-modify-write; the neutral-delete rule survives, including the two-copy case.
- `BufferCache` still keys on path, with a test and a doc comment saying why.
- Export filenames carry `_N`, composing with the existing `-N` rather than replacing it.
- The folder-open cost measured and recorded.
- `cargo fmt --check`, `clippy --all-targets --all-features -D warnings`, `cargo test --all` all green **by exit code**; 47 goldens untouched.
- No GUI change — the Library, sync and the menu are plan 2.
