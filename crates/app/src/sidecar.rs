//! The `<file>.json` edit sidecar.
//!
//! Extracted from `main.rs` so `sync` can reach it and so it is testable at all.

use edit_model::{EditLoadError, Edits};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The sidecar path for a source file: the full filename with `.json` **appended**
/// (`photo.RAF` → `photo.RAF.json`), not the extension replaced — so a RAW and a JPEG
/// sharing a basename get distinct sidecars.
pub fn path(photo: &Path) -> PathBuf {
    let mut s = photo.as_os_str().to_os_string();
    s.push(".json");
    PathBuf::from(s)
}

/// A photo's stored edits — **the original's**, copy 0.
///
/// `None` when there is no sidecar, it is malformed, or nothing is stored for copy 0. A
/// hand-edited file with a typo should read as "no edits", not take anything down.
///
/// ⚠⚠ This delegates to the copy-aware API rather than parsing the file itself, and that is not
/// tidiness. Once the sidecar became an envelope, a reader that parsed it as a bare `Edits`
/// returned `None` for **every** file written by the new code — which silently reset a synced
/// photo to neutral. Two readers of one format is the trap; there is now one.
pub fn load(photo: &Path) -> Option<Edits> {
    load_copy(photo, 0)
}

/// Write the original's edits, leaving every other copy alone.
pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    let mut s = load_all(photo);
    s.set_edits(0, edits.clone());
    save_all(photo, &s)
}

/// Persist a photo's edits, **removing** the sidecar when they are neutral.
///
/// ⚠ Writing nothing and writing nothing *new* are not the same thing. A photo reset to
/// neutral must have its sidecar deleted, or the old edits are loaded straight back the next
/// time it is opened — which is exactly what "Reset all, switch away, switch back" did.
pub fn persist(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    persist_copy(photo, 0, edits)
}

/// Delete a photo's sidecar. Absent is not an error.
pub fn remove(photo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(photo)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// One saved state within a copy — a named point in that copy's editing.
///
/// ⚠⚠ **Deliberately outside `History<Edits>`, and joined to it at exactly one point.** Taking
/// one changes no `Edits`, so there is nothing for an undo stack of `Edits` to hold; *restoring*
/// one does change them, so `History::push` covers it and Ctrl+Z undoes a restore for free.
/// Undoing past a snapshot cannot touch it, because it was never in the stack. That independence
/// is what makes a snapshot unable to vanish silently — which is why this replaced the dropped
/// "persistent undo history". See `docs/superpowers/specs/2026-08-27-snapshots-design.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Identity: `max + 1` within its copy, **never reused**.
    ///
    /// ⚠ The copy-number rule, for the copy-number reason: an *unnamed* snapshot's label is
    /// derived from this, so reusing a number would silently rename an existing row.
    pub n: u32,
    /// What it is called, when it has been given a name. `None` follows the number.
    #[serde(default)]
    pub name: Option<String>,
    /// When it was taken — **local wall-clock seconds** since the epoch.
    ///
    /// ⚠ The same convention `Capture::taken` stores its date under, and for the same two
    /// reasons: rendering it needs no timezone, and wall-clock is what a person means by "when
    /// I took it".
    pub taken: i64,
    pub edits: Edits,
}

impl Snapshot {
    /// What this snapshot is called on screen.
    ///
    /// ⚠ **The one rule, with every consumer going through it** — the strip's row, the rename
    /// field's initial text, the name seeded into a virtual copy. That is `entries::shown_name`'s
    /// rule and the reason it exists: this project has already shipped a bug where the Library
    /// printed `file_name` while a copy's real name lived somewhere else, because two
    /// implementations of "what is this called" existed.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Snapshot {}", self.n))
    }
}

/// One version of a photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyEntry {
    /// `0` is the original; copies are numbered from 1 and never reused.
    pub copy: u32,
    /// What this copy is called, when it has been given a name.
    ///
    /// ⚠⚠ **Envelope metadata, never an `Edits` field, and that is load-bearing.** In `Edits` it
    /// would need an edit group and would then **travel with a paste, a preset or a sync** —
    /// pasting a B&W recipe onto another photo would rename it "B&W" — and "Reset adjustments"
    /// would wipe it. The cost is that `History<Edits>` cannot undo it, which is why renaming
    /// gets its own `ActionKind`.
    ///
    /// ⚠ Only copies carry one. Copy 0 *is* the file, so its name is the filename on disk.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub edits: Edits,
}

/// Everything stored for one file.
///
/// ⚠⚠ **Deliberately NOT `#[serde(default)]`, and that omission is load-bearing.** `copies` being
/// a *required* field is what stops a legacy bare-`Edits` sidecar from ever being mistaken for an
/// envelope. Adding the attribute here — the obvious "forward compatibility" move — combined with
/// an untagged dispatch in `load_all` is what silently reads every existing sidecar as an empty
/// copy list. Verified by doing exactly that: untagged **alone** is harmless and every test
/// passes; untagged **plus** this attribute loses the edits.
///
/// `load_all`'s explicit dispatch means adding it would be safe today. Both guards are kept
/// because either one alone is enough, and neither is obvious to the next reader.
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
        self.copies
            .iter()
            .map(|c| c.copy)
            .max()
            .map_or(1, |m| m + 1)
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

    /// Replace (or add) one copy's name, leaving its edits alone.
    pub fn set_name(&mut self, copy: u32, name: Option<String>) {
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => c.name = name,
            None => self.copies.push(CopyEntry {
                copy,
                name,
                edits: Edits::default(),
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Renumber the copies so they run 1, 2, 3… with no gaps, and report what moved.
    ///
    /// ⚠⚠ **This deliberately breaks the "copy numbers are never reused" invariant, and that is
    /// why it is a button rather than something that happens on its own.** Numbers are stable
    /// precisely so a copy's export filename and its undo stack cannot shift under it; the cost
    /// of that is `_1, _3, _7` after some deleting, which is the mess Reindex exists to tidy. An
    /// explicit action the user asked for is a different thing from a silent renumber.
    ///
    /// ⚠ A **named** copy keeps its name, so its export filename does not move — only unnamed
    /// copies, whose name is derived from the number, are renamed by this.
    ///
    /// ⚠ The caller must re-key or drop anything keyed on `PhotoId`, above all the history
    /// stash: without that, old copy 1's undo stack would attach to whatever becomes the new
    /// copy 1.
    pub fn compact(&mut self) -> Vec<(u32, u32)> {
        self.copies.sort_by_key(|c| c.copy);
        let mut moves = Vec::new();
        let mut next = 1;
        for c in self.copies.iter_mut() {
            // ⚠ Copy 0 *is* the file and is never renumbered — there is nothing else it could be.
            if c.copy == 0 {
                continue;
            }
            if c.copy != next {
                moves.push((c.copy, next));
                c.copy = next;
            }
            next += 1;
        }
        moves
    }

    pub fn name_of(&self, copy: u32) -> Option<&str> {
        self.get(copy).and_then(|c| c.name.as_deref())
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

/// When `photo`'s edits were last written, or `None` if it has never been edited.
///
/// ⚠ The sidecar's modification time is what "last edited" means here — there is no edit
/// timestamp stored inside it, and adding one would be a format change for something the
/// filesystem already records.
///
/// ⚠ A `stat` per photo, not a read: the sidecar's *contents* cost 4.6 ms for 830 files, and
/// asking for the timestamp alone is far less than that.
pub fn edited_at(photo: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path(photo)).ok()?.modified().ok()
}

/// Every version stored for `photo`.
///
/// ⚠⚠ **The dispatch is explicit rather than `#[serde(untagged)]`, and the reason is narrower
/// than it first looks — measured, not assumed.**
///
/// An untagged enum of `{ Envelope, Legacy }` is *by itself* safe here, because `Sidecar::copies`
/// is a required field and a legacy bare-`Edits` object therefore fails to match `Envelope`. But
/// it becomes silent data loss the moment anyone adds `#[serde(default)]` to `Sidecar` — at which
/// point every legacy sidecar matches `Envelope` with an empty copy list and every edit anyone
/// ever made is discarded, with a green test suite.
///
/// Both halves were verified by sabotage: untagged alone passed all 15 tests, untagged plus the
/// attribute failed `a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact`.
/// Explicit dispatch is immune to that combination, which is why it is worth the four extra
/// lines. The same class of trap already cost this project a test that passed while reading
/// nothing (`an_existing_linear_sidecar_still_loads`, M3.1b).
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

/// One copy's stored edits, or `None` when that copy has none.
pub fn load_copy(photo: &Path, copy: u32) -> Option<Edits> {
    load_all(photo).get(copy).map(|c| c.edits.clone())
}

/// Persist one copy's edits, leaving every other copy — and this copy's name — untouched.
///
/// ⚠ Read-modify-write, because several versions share one file. An overwrite would drop the
/// other copies, and it would drop this copy's name, which is not an edit.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        /// ⚠ The tag is **not** relied on for uniqueness. Two tests that happened to pick the
        /// same tag would destroy each other's directory here — `remove_dir_all` runs first, and
        /// `cargo test` runs tests in parallel. That already happened once while writing the
        /// virtual-copies tests, and it surfaced as an unrelated pre-existing test failing during
        /// a sabotage run, which is the worst possible time to be chasing a phantom.
        ///
        /// Uniqueness is structural instead — process id plus a monotonic counter — the same
        /// remedy `image-decode`'s `temp_path` needed after taking uniqueness from the clock.
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("rustroom_sidecar_{tag}_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_path_appends_json_to_the_whole_filename() {
        // Not "replace the extension": a RAW and a JPEG sharing a basename must get
        // distinct sidecars.
        assert_eq!(
            path(Path::new("/a/photo.RAF")),
            PathBuf::from("/a/photo.RAF.json")
        );
        assert_eq!(
            path(Path::new("/a/photo.jpg")),
            PathBuf::from("/a/photo.jpg.json")
        );
        assert_ne!(path(Path::new("/a/p.RAF")), path(Path::new("/a/p.jpg")));
    }

    #[test]
    fn save_then_load_round_trips() {
        let d = TempDir::new("roundtrip");
        let photo = d.photo("x.RAF");
        let e = Edits {
            exposure: 1.5,
            ..Edits::default()
        };
        save(&photo, &e).unwrap();
        assert_eq!(load(&photo), Some(e));
    }

    #[test]
    fn persisting_neutral_edits_deletes_the_sidecar() {
        // ⚠ The "Reset all" bug: `save` was skipped for neutral edits but the old file was
        // left behind, so switching away and back loaded the edits straight back in.
        let d = TempDir::new("persist_neutral");
        let photo = d.photo("x.RAF");
        persist(
            &photo,
            &Edits {
                exposure: 1.5,
                ..Edits::default()
            },
        )
        .unwrap();
        assert!(path(&photo).exists(), "a non-neutral edit should be stored");

        persist(&photo, &Edits::default()).unwrap();
        assert!(
            !path(&photo).exists(),
            "resetting to neutral must delete the sidecar, not leave a stale one"
        );
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn persisting_neutral_edits_with_no_sidecar_is_not_an_error() {
        // The common case: open a photo, change nothing, switch away.
        let d = TempDir::new("persist_none");
        persist(&d.photo("x.RAF"), &Edits::default()).unwrap();
    }

    #[test]
    fn an_absent_or_malformed_sidecar_loads_as_none() {
        // The same forgiving posture the preset and settings stores take.
        let d = TempDir::new("absent");
        let photo = d.photo("x.RAF");
        assert_eq!(load(&photo), None);
        std::fs::write(path(&photo), "{ not json").unwrap();
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn removing_an_absent_sidecar_is_not_an_error() {
        let d = TempDir::new("remove");
        let photo = d.photo("x.RAF");
        remove(&photo).unwrap();
        save(&photo, &Edits::default()).unwrap();
        assert!(path(&photo).exists());
        remove(&photo).unwrap();
        assert!(!path(&photo).exists());
    }

    #[test]
    fn a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact() {
        // ⚠⚠ THE test of this feature: every sidecar written before today is a bare `Edits`
        // object, and reading one as an empty copy list would discard a user's entire edit
        // history without failing anything.
        //
        // ⚠ It asserts the **payload**, not the variant — M3.1b's
        // `an_existing_linear_sidecar_still_loads` asserted only the variant and passed while
        // reading nothing at all. Watched failing against an untagged dispatch combined with
        // `#[serde(default)]` on `Sidecar`; see `load_all` for why it takes both.
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
                    name: None,
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
        for n in 0..=2 {
            s.copies.push(CopyEntry {
                copy: n,
                name: None,
                edits: Edits::default(),
            });
        }
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
    /// A sidecar with copy 0 at `e0` and copy 1 at `e1`.
    fn two_copies(photo: &Path, e0: f32, e1: f32) {
        save_all(
            photo,
            &Sidecar {
                copies: vec![
                    CopyEntry {
                        copy: 0,
                        name: None,
                        edits: Edits {
                            exposure: e0,
                            ..Edits::default()
                        },
                    },
                    CopyEntry {
                        copy: 1,
                        name: None,
                        edits: Edits {
                            exposure: e1,
                            ..Edits::default()
                        },
                    },
                ],
            },
        )
        .unwrap();
    }

    #[test]
    fn persisting_one_copy_leaves_the_others_alone() {
        // ⚠ Read-modify-write, not overwrite. Two copies of one file live in one file, so
        // saving one must not drop the other.
        let dir = TempDir::new("persist_one");
        let photo = dir.0.join("a.RAF");
        two_copies(&photo, 1.0, 5.0);

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
        assert_eq!(s.get(1).unwrap().edits.exposure, 5.0, "copy 1 lost");
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
        two_copies(&photo, 1.0, 2.0);
        persist_copy(&photo, 0, &Edits::default()).unwrap();
        assert!(path(&photo).exists(), "copy 1 must survive");
        assert_eq!(load_all(&photo).get(1).unwrap().edits.exposure, 2.0);
    }

    #[test]
    fn loading_one_copy_returns_only_that_copys_edits() {
        let dir = TempDir::new("load_one");
        let photo = dir.0.join("a.RAF");
        two_copies(&photo, 1.0, 2.0);
        assert_eq!(load_copy(&photo, 0).unwrap().exposure, 1.0);
        assert_eq!(load_copy(&photo, 1).unwrap().exposure, 2.0);
        assert!(load_copy(&photo, 9).is_none(), "an absent copy is None");
    }
}

#[cfg(test)]
mod naming_tests {
    use super::*;

    fn edited(v: f32) -> Edits {
        Edits {
            exposure: v,
            ..Edits::default()
        }
    }

    #[test]
    fn naming_a_copy_leaves_every_edit_and_the_other_copies_alone() {
        // ⚠⚠ This is the guard for what was reported: renaming a copy used to rename the *file*,
        // which every sibling copy shares. A name now touches exactly one copy's `name` and
        // nothing else — not the edits, not the siblings, not the file.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(1, edited(2.0));
        s.set_edits(2, edited(3.0));

        s.set_name(1, Some("Warm".into()));

        assert_eq!(s.name_of(1), Some("Warm"));
        assert_eq!(s.name_of(0), None, "the original is untouched");
        assert_eq!(s.name_of(2), None, "so is the sibling");
        assert_eq!(s.get(0).unwrap().edits, edited(1.0));
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(2.0),
            "its own edits survive"
        );
        assert_eq!(s.get(2).unwrap().edits, edited(3.0));
    }

    #[test]
    fn clearing_a_name_puts_the_copy_back_to_its_number() {
        let mut s = Sidecar::default();
        s.set_edits(1, edited(2.0));
        s.set_name(1, Some("Warm".into()));
        s.set_name(1, None);
        assert_eq!(s.name_of(1), None);
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(2.0),
            "undo must not reset edits"
        );
    }

    #[test]
    fn a_named_copy_keeps_the_sidecar_alive_even_when_neutral() {
        // ⚠ Otherwise naming a copy and then resetting it would delete the copy, because
        // `persist_all` removes a sidecar it considers neutral.
        let mut s = Sidecar::default();
        s.set_name(0, Some("named".into()));
        assert!(
            !s.is_neutral(),
            "a name is worth a file even with neutral edits"
        );
    }

    #[test]
    fn setting_edits_does_not_clear_a_name() {
        // ⚠ Every slider drag calls `set_edits`. Clearing the name there would erase it on the
        // next keystroke after renaming — invisible until the export came out wrong.
        let mut s = Sidecar::default();
        s.set_name(1, Some("Warm".into()));
        s.set_edits(1, edited(2.0));
        assert_eq!(s.name_of(1), Some("Warm"));
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    fn edited(v: f32) -> Edits {
        Edits {
            exposure: v,
            ..Edits::default()
        }
    }

    #[test]
    fn deleting_the_first_copy_makes_the_second_copy_one_again() {
        // ⚠ The case this exists for: two copies, delete the first, and `_2` is left with no
        // `_1` beside it.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(2, edited(3.0));

        let moves = s.compact();

        assert_eq!(moves, vec![(2, 1)]);
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(3.0),
            "it kept its own edits"
        );
        assert!(s.get(2).is_none(), "and vacated its old number");
        assert_eq!(
            s.get(0).unwrap().edits,
            edited(1.0),
            "the original is untouched"
        );
    }

    #[test]
    fn already_contiguous_copies_do_not_move() {
        // ⚠ An empty move list is what lets the caller skip writing the file and dropping undo
        // stacks for nothing.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(1, edited(2.0));
        s.set_edits(2, edited(3.0));
        assert!(s.compact().is_empty());
        assert_eq!(s.get(1).unwrap().edits, edited(2.0));
        assert_eq!(s.get(2).unwrap().edits, edited(3.0));
    }

    #[test]
    fn copies_keep_their_order_and_their_names() {
        // ⚠ Ascending by old number, so the *order* people see never changes — only the gaps
        // close. Renumbering that also reordered would move photos around the grid.
        let mut s = Sidecar::default();
        s.set_edits(3, edited(3.0));
        s.set_edits(7, edited(7.0));
        s.set_name(7, Some("Warm".into()));
        s.set_edits(5, edited(5.0));

        assert_eq!(s.compact(), vec![(3, 1), (5, 2), (7, 3)]);

        assert_eq!(s.get(1).unwrap().edits, edited(3.0));
        assert_eq!(s.get(2).unwrap().edits, edited(5.0));
        assert_eq!(s.get(3).unwrap().edits, edited(7.0));
        assert_eq!(
            s.name_of(3),
            Some("Warm"),
            "a named copy keeps its name, so its export filename does not move"
        );
    }

    #[test]
    fn a_file_with_no_stored_original_still_numbers_its_copies_from_one() {
        // ⚠ A sidecar can hold copies and no copy 0 — `persist_copy(.., 1, ..)` on an unedited
        // photo does exactly that. Numbering from 0 here would make a copy claim to be the file.
        let mut s = Sidecar::default();
        s.set_edits(4, edited(4.0));
        assert_eq!(s.compact(), vec![(4, 1)]);
        assert!(s.get(0).is_none());
        assert_eq!(s.get(1).unwrap().edits, edited(4.0));
    }

    #[test]
    fn the_next_copy_number_follows_a_compaction() {
        // ⚠ `next_copy` is `max + 1`; after compacting it must not hand out a number that is
        // already taken, or a new copy would overwrite an existing one.
        let mut s = Sidecar::default();
        s.set_edits(3, edited(3.0));
        s.set_edits(9, edited(9.0));
        s.compact();
        assert_eq!(s.next_copy(), 3);
        assert!(s.get(3).is_none(), "and that number really is free");
    }
}

/// Snapshots: labelling, storage, numbering and the neutrality rule.
///
/// ⚠ Its own module rather than more of `tests`, following this file's existing partition —
/// `naming_tests` and `compact_tests` are organised the same way.
#[cfg(test)]
mod snapshot_tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — never the tag alone. Two
    /// tests sharing a tag destroy each other's directory here, because `remove_dir_all` runs
    /// first and `cargo test` runs in parallel. That has already happened once in this file.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("rustroom_snap_{tag}_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snap(n: u32, exposure: f32) -> Snapshot {
        Snapshot {
            n,
            name: None,
            taken: 0,
            edits: Edits {
                exposure,
                ..Edits::default()
            },
        }
    }

    #[test]
    fn an_unnamed_snapshot_is_labelled_by_its_number() {
        assert_eq!(snap(3, 0.0).label(), "Snapshot 3");
    }

    #[test]
    fn a_named_snapshot_is_labelled_by_its_name() {
        let mut s = snap(3, 0.0);
        s.name = Some("Before the crop".to_string());
        assert_eq!(s.label(), "Before the crop");
    }
}
