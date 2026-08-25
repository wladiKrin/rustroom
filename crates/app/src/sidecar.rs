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

/// A photo's stored edits. `None` when there is no sidecar **or it is malformed** — a
/// hand-edited file with a typo should read as "no edits", not take anything down.
pub fn load(photo: &Path) -> Option<Edits> {
    Edits::from_json_file(&path(photo)).ok()
}

pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    edits.to_json_file(&path(photo))
}

/// Persist a photo's edits, **removing** the sidecar when they are neutral.
///
/// ⚠ Writing nothing and writing nothing *new* are not the same thing. A photo reset to
/// neutral must have its sidecar deleted, or the old edits are loaded straight back the next
/// time it is opened — which is exactly what "Reset all, switch away, switch back" did.
pub fn persist(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    if *edits == Edits::default() {
        return remove(photo).map_err(EditLoadError::from);
    }
    save(photo, edits)
}

/// Delete a photo's sidecar. Absent is not an error.
pub fn remove(photo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(photo)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

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
}
