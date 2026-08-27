//! Turning a folder scan into the rows the Library shows.
//!
//! ⚠ One row per `(file, copy)`, not per file. The expansion is a pure function with the
//! sidecar loader **injected**, so every rule below is tested without touching disk or a GUI —
//! deliberately, because this is the code path that has produced three sparse-index bugs.

use crate::photo_id::PhotoId;
use crate::sidecar::Sidecar;
use catalog::CatalogEntry;
use std::path::Path;

/// One row in the Library: a file, which copy of it, and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    pub id: PhotoId,
    /// The file's own name, shared by every copy of it.
    pub file_name: String,
    /// This copy's own name, when it has one. Always `None` on the original, whose name *is*
    /// `file_name`.
    pub name: Option<String>,
}

impl PhotoEntry {
    /// What the grid, filmstrip and Develop footer show.
    ///
    /// ⚠⚠ **A copy shows its own name, not the file's name plus a marker.** An unnamed copy's
    /// name is `{stem}_{copy}.{ext}` — a whole filename, derived rather than stored, and exactly
    /// what it will export as. So the label, the editable field and the export filename are one
    /// string, and cannot drift apart.
    ///
    /// ⚠ There is exactly one editable name per row, and *which* name it is depends on the row:
    /// the original's is the filename on disk, a copy's is this. That asymmetry is the feature —
    /// a copy has no file of its own, so renaming one used to rename the shared file and drag
    /// every other copy along with it.
    pub fn label(&self) -> String {
        shown_name(&self.file_name, self.id.copy, self.name.as_deref())
    }
}

/// What a photo is called, from its file's name, its copy number and its own name.
///
/// ⚠ **One implementation, three consumers**: the Library caption, the editable field in both
/// views, and the export filename's default all come from here. They had briefly grown two
/// copies of this rule, which is precisely how the grid ended up showing a file's name where a
/// copy's name belonged.
pub fn shown_name(file_name: &str, copy: u32, name: Option<&str>) -> String {
    match (copy, name) {
        // ⚠ Copy 0 *is* the file, so its name is the filename and a stored one is ignored.
        (0, _) => file_name.to_string(),
        (_, Some(n)) => n.to_string(),
        (c, None) => crate::file_rename::default_copy_name(file_name, c),
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
                // ⚠ Never on the original: copy 0's name is the file's, and holding a second one
                // beside it would be two names for one thing.
                name: (n != 0).then(|| s.name_of(n).map(str::to_string)).flatten(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::CopyEntry;
    use edit_model::Edits;
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
            name: name.map(str::to_string),
            edits: Edits::default(),
            snapshots: Vec::new(),
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
    fn the_label_is_the_filename_plus_the_copy_number() {
        // ⚠ One number used everywhere: the identity, the default label and the default export
        // name are the same value. The original carries no number at all.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, None)],
        });
        assert_eq!(got[0].label(), "a.RAF");
        assert_eq!(got[1].label(), "a_1.RAF");
        assert_eq!(got[2].label(), "a_2.RAF");
    }

    #[test]
    fn renaming_the_file_moves_every_unnamed_copy_with_it() {
        // ⚠⚠ The bug this pins: after renaming the original, only *some* copies appeared to
        // follow. Every unnamed copy derives its name from the file's, so they must all move
        // together — and a named one must not, because its name is its own.
        let before = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![
                copy(0, None),
                copy(1, None),
                copy(2, None),
                copy(3, Some("Warm")),
            ],
        });
        let after = build(&scan(&["b.RAF"]), |_| Sidecar {
            copies: vec![
                copy(0, None),
                copy(1, None),
                copy(2, None),
                copy(3, Some("Warm")),
            ],
        });
        let labels = |v: &[PhotoEntry]| v.iter().map(|e| e.label()).collect::<Vec<_>>();
        assert_eq!(labels(&before), ["a.RAF", "a_1.RAF", "a_2.RAF", "Warm"]);
        assert_eq!(
            labels(&after),
            ["b.RAF", "b_1.RAF", "b_2.RAF", "Warm"],
            "every unnamed copy follows the file; the named one keeps its own name"
        );
    }

    #[test]
    fn an_unnamed_copy_is_labelled_like_a_filename() {
        // ⚠ Not "a.RAF / 1". The label *is* what the copy exports as, so it reads as a filename.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None)],
        });
        assert_eq!(got[0].label(), "a.RAF");
        assert_eq!(got[1].label(), "a_1.RAF");
    }

    #[test]
    fn a_named_copy_shows_its_name_instead_of_its_number() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, Some("B&W"))],
        });
        assert_eq!(
            got[1].label(),
            "a_1.RAF",
            "unnamed falls back to the number"
        );
        assert_eq!(got[2].label(), "B&W");
        assert!(
            !got[2].label().contains("a.RAF"),
            "the name replaces the filename, it does not extend it"
        );
    }

    #[test]
    fn the_original_never_carries_a_copy_name() {
        // ⚠ Copy 0 *is* the file, so its name is the filename. A name stored against it — which a
        // hand-edited sidecar can hold — must not become a second name shown beside the first.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, Some("nonsense")), copy(1, Some("B&W"))],
        });
        assert_eq!(got[0].name, None);
        assert_eq!(got[0].label(), "a.RAF");
        assert_eq!(got[1].name.as_deref(), Some("B&W"));
    }
}
