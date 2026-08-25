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
    /// This copy's user-given name, if it has one.
    pub name: Option<String>,
}

impl PhotoEntry {
    /// What the grid, filmstrip and Develop footer show.
    ///
    /// ⚠ A named copy shows its name *instead of* its number, not as well as it. Two labels for
    /// one thing is how a display value drifts from the identity it is meant to describe.
    pub fn label(&self) -> String {
        match (&self.name, self.id.copy) {
            (_, 0) => self.file_name.clone(),
            (Some(n), _) => format!("{} / {n}", self.file_name),
            (None, c) => format!("{} / {c}", self.file_name),
        }
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
                name: s.get(n).and_then(|c| c.name.clone()),
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
            })
            .collect()
    }

    fn copy(n: u32, name: Option<&str>) -> CopyEntry {
        CopyEntry {
            copy: n,
            name: name.map(str::to_string),
            edits: Edits::default(),
        }
    }

    #[test]
    fn a_file_with_no_sidecar_is_one_row() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar::default());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.copy, 0);
        assert_eq!(got[0].file_name, "a.RAF");
        assert_eq!(got[0].name, None);
    }

    #[test]
    fn the_original_is_always_a_row_even_when_the_sidecar_omits_it() {
        // ⚠ A sidecar can hold copy 1 and no copy 0 — `persist_copy(.., 1, ..)` on a photo whose
        // original was never edited does exactly that. The original is the *file*; it exists
        // whether or not anything is stored for it, and omitting it would make a photo vanish
        // from the browser after making a copy of it.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(1, Some("B&W"))],
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
    fn a_copys_name_comes_from_the_sidecar() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, Some("B&W"))],
        });
        assert_eq!(got[0].name, None);
        assert_eq!(got[1].name.as_deref(), Some("B&W"));
    }

    #[test]
    fn the_label_falls_back_to_the_copy_number_when_unnamed() {
        // ⚠ One number used everywhere: the identity, the label and the export suffix are the
        // same value. A named copy shows its name *instead of* the number, not as well as.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, Some("B&W"))],
        });
        assert_eq!(got[0].label(), "a.RAF");
        assert_eq!(got[1].label(), "a.RAF / 1");
        assert_eq!(got[2].label(), "a.RAF / B&W");
    }
}
