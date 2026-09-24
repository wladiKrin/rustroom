//! Turning a folder scan into the rows the Library shows.
//!
//! ⚠ One row per `(file, copy)`, not per file. The expansion is a pure function with the
//! sidecar loader **injected**, so every rule below is tested without touching disk or a GUI —
//! deliberately, because this is the code path that has produced three sparse-index bugs.

use crate::photo_id::PhotoId;
use crate::sidecar::Sidecar;
use catalog::CatalogEntry;
use edit_model::Edits;
use std::path::Path;

/// One row in the Library: a file, which copy of it, and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    pub id: PhotoId,
    /// The file's own name, shared by every copy of it.
    pub file_name: String,
    /// This copy's label, when it has one — "B&W", "Client crop".
    ///
    /// ⚠ **Not a filename**, and any copy may carry one including the original. See
    /// `sidecar::CopyEntry::label`.
    pub label: Option<String>,
    /// This copy's stored edits, read from the sidecar when the row was built.
    ///
    /// ⚠ Carried so a thumbnail can be *rendered* with them without re-reading the sidecar per
    /// frame. `build` already loads the whole envelope — measured at 4.6 ms for 830 files — so
    /// this costs nothing new.
    ///
    /// ⚠⚠ **Stale for the open photo while it is being edited**, because `persist_current_session`
    /// writes only on a photo switch or on quit. The caller substitutes `Session::edits` for that
    /// one photo — see `RustroomApp::thumb_edits`.
    pub edits: Edits,
}

impl PhotoEntry {
    /// The caption printed under this row.
    pub fn caption(&self) -> String {
        caption(&self.file_name, self.id.copy)
    }

    /// This row's badge text, if it has one — see [`label_of`].
    pub fn badge(&self) -> Option<String> {
        label_of(self.id.copy, self.label.as_deref())
    }
}

/// What the Library grid, the filmstrip and the Develop footer print under a photo.
///
/// ⚠⚠ **The file's own name, on every row including a copy's.** One rule, no special case —
/// which is what lets a copy show which photograph it belongs to. Before M6.0 a copy showed its
/// *own* name instead, so a labelled copy gave no sign of where it came from.
///
/// A copy's own text lives in [`label_of`] and is drawn in the badge.
pub fn caption(file_name: &str, _copy: u32) -> String {
    file_name.to_string()
}

/// What the copy badge carries: the label, else the copy number, else nothing.
///
/// ⚠ The ordering is the whole rule, and step 1 applies to **copy 0 too** — a labelled original
/// badges "Colour". Only an *unlabelled* original has nothing to say.
pub fn label_of(copy: u32, label: Option<&str>) -> Option<String> {
    match (copy, label) {
        (_, Some(l)) if !l.is_empty() => Some(l.to_string()),
        (0, _) => None,
        (c, _) => Some(c.to_string()),
    }
}

/// Expand a folder scan into one row per `(file, copy)`.
///
/// `load` is injected so the rules here are testable without disk. In production it is
/// `sidecar::load_all`; measured at 4.6 ms for 830 files, so it stays inline rather than moving
/// to a worker thread.
pub fn build(scan: &[CatalogEntry], load: impl Fn(&Path) -> Sidecar) -> Vec<PhotoEntry> {
    let mut out = Vec::with_capacity(scan.len());
    for e in scan {
        let s = load(&e.path);
        // ⚠ The original is always a row, whether or not the sidecar mentions it — it *is* the
        // file. A sidecar can legitimately hold copy 1 and no copy 0.
        let mut numbers: Vec<u32> = s.copies.iter().map(|c| c.copy).collect();
        if !numbers.contains(&0) {
            numbers.push(0);
        }
        numbers.sort_unstable();
        numbers.dedup();
        for n in numbers {
            out.push(PhotoEntry {
                id: PhotoId::new(e.path.clone(), n),
                file_name: e.file_name.clone(),
                // ⚠ Any copy may carry a label, the original included — a label is not a
                // filename, so the copy-0 exception that used to sit here has no premise left.
                label: s.label_of(n).map(str::to_string),
                // ⚠ `Edits::default()` for a row the sidecar does not mention — the original of
                // an unedited photo, which is neutral by definition.
                edits: s.get(n).map(|c| c.edits.clone()).unwrap_or_default(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::CopyEntry;
    use std::path::PathBuf;

    fn scan(names: &[&str]) -> Vec<CatalogEntry> {
        names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(format!("/x/{n}")),
                file_name: n.to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect()
    }

    fn copy(n: u32, name: Option<&str>) -> CopyEntry {
        CopyEntry {
            copy: n,
            label: name.map(str::to_string),
            edits: Edits::default(),
            snapshots: Vec::new(),
            snapshot_hwm: 0,
            uploads: Vec::new(),
        }
    }

    #[test]
    fn a_file_with_no_sidecar_is_one_row() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar::default());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.copy, 0);
        assert_eq!(got[0].file_name, "a.RAF");
    }

    #[test]
    fn the_original_is_always_a_row_even_when_the_sidecar_omits_it() {
        // ⚠ A sidecar can hold copy 1 and no copy 0 — `persist_copy(.., 1, ..)` on a photo whose
        // original was never edited does exactly that. The original is the *file*; it exists
        // whether or not anything is stored for it, and omitting it would make a photo vanish
        // from the browser after making a copy of it.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(1, None)],
            copy_hwm: 0,
        });
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id.copy, 0, "the original comes first");
        assert_eq!(got[1].id.copy, 1);
    }

    #[test]
    fn copies_are_adjacent_to_their_file_and_ascending() {
        let got = build(&scan(&["a.RAF", "b.RAF"]), |p| {
            if p.ends_with("a.RAF") {
                Sidecar {
                    copies: vec![copy(3, None), copy(0, None), copy(1, None)],
                    copy_hwm: 0,
                }
            } else {
                Sidecar::default()
            }
        });
        let shape: Vec<(String, u32)> = got
            .iter()
            .map(|e| (e.file_name.clone(), e.id.copy))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("a.RAF".to_string(), 0),
                ("a.RAF".to_string(), 1),
                ("a.RAF".to_string(), 3),
                ("b.RAF".to_string(), 0),
            ]
        );
    }

    #[test]
    fn every_copy_is_captioned_with_the_file_name() {
        // ⚠⚠ Replaces `the_label_is_the_filename_plus_the_copy_number`, which asserted the rule
        // M6.0 removes. A copy used to be captioned `a_1.RAF` — its own derived filename — so it
        // gave no sign of which photograph it came from. The number moved to the badge.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, None)],
            copy_hwm: 0,
        });
        assert_eq!(got[0].caption(), "a.RAF");
        assert_eq!(got[1].caption(), "a.RAF");
        assert_eq!(got[2].caption(), "a.RAF");
        assert_eq!(
            got[0].badge(),
            None,
            "an unlabelled original has nothing to badge"
        );
        assert_eq!(got[1].badge(), Some("1".into()));
        assert_eq!(got[2].badge(), Some("2".into()));
    }

    #[test]
    fn renaming_the_file_moves_every_copy_with_it() {
        // ⚠⚠ The bug this pins: after renaming the original, only *some* copies appeared to
        // follow. Under M6.0 the caption is the file's name, so **every** copy follows including
        // a labelled one — its label is not a filename and is unaffected by the rename.
        let cs = || {
            vec![
                copy(0, None),
                copy(1, None),
                copy(2, None),
                copy(3, Some("Warm")),
            ]
        };
        let before = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: cs(),
            copy_hwm: 0,
        });
        let after = build(&scan(&["b.RAF"]), |_| Sidecar {
            copies: cs(),
            copy_hwm: 0,
        });
        let caps = |v: &[PhotoEntry]| v.iter().map(|e| e.caption()).collect::<Vec<_>>();
        assert_eq!(caps(&before), ["a.RAF", "a.RAF", "a.RAF", "a.RAF"]);
        assert_eq!(caps(&after), ["b.RAF", "b.RAF", "b.RAF", "b.RAF"]);
        let badges = |v: &[PhotoEntry]| v.iter().map(|e| e.badge()).collect::<Vec<_>>();
        assert_eq!(
            badges(&after),
            [
                None,
                Some("1".into()),
                Some("2".into()),
                Some("Warm".into())
            ],
            "the label survives a file rename; the numbers are unchanged"
        );
    }

    #[test]
    fn a_labelled_copy_badges_its_label_instead_of_its_number() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, Some("B&W"))],
            copy_hwm: 0,
        });
        assert_eq!(
            got[1].badge(),
            Some("1".into()),
            "unlabelled falls back to the number"
        );
        assert_eq!(got[2].badge(), Some("B&W".into()));
        assert_eq!(
            got[2].caption(),
            "a.RAF",
            "the label goes in the badge; it does not replace the caption"
        );
    }

    #[test]
    fn the_original_may_carry_a_label_too() {
        // ⚠⚠ Replaces `the_original_never_carries_a_copy_name`. That exception existed because a
        // copy-0 "name" would have been a second filename; a label is not a filename, so the
        // premise is gone and "Colour / B&W" is expressible.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, Some("Colour")), copy(1, Some("B&W"))],
            copy_hwm: 0,
        });
        assert_eq!(got[0].caption(), "a.RAF");
        assert_eq!(got[0].badge(), Some("Colour".into()));
        assert_eq!(got[1].badge(), Some("B&W".into()));
    }

    #[test]
    fn a_labelled_copy_captions_with_its_file_name_and_badges_its_label() {
        // ⚠ The pair that matters: the caption says which photograph, the badge says which
        // version. Before M6.0 one string tried to do both and the caption lost.
        let got = build(&scan(&["DSCF1234.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(2, Some("B&W"))],
            copy_hwm: 0,
        });
        assert_eq!(got[1].caption(), "DSCF1234.RAF");
        assert_eq!(got[1].badge(), Some("B&W".into()));
    }
}
