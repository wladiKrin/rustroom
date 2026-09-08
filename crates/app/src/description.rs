//! The Information dialog's editable description: a draft, and the one rule about committing it.
//!
//! ⚠ No egui here — a lib module, so the commit rule is testable without a window. The dialog
//! (`modals/info.rs`) owns the fields; this owns *when a write happens*.

use catalog::db::{Catalog, Description};
use std::path::Path;

/// What the dialog opened with, and what it holds now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DescriptionDraft {
    pub original: Description,
    pub edited: Description,
}

impl DescriptionDraft {
    /// Read `photo`'s description once, for the dialog to edit.
    ///
    /// ⚠ Called on the dialog's first frame and cached in the modal — a catalog read in a body
    /// that runs sixty times a second is the `apply_view_demosaic` mistake.
    pub fn open(cat: Option<&Catalog>, photo: &Path) -> Self {
        let original = cat
            .and_then(|c| c.description_of(photo).ok().flatten())
            .unwrap_or_default();
        Self {
            edited: original.clone(),
            original,
        }
    }

    pub fn changed(&self) -> bool {
        self.edited != self.original
    }
}

/// Write the draft if it changed. Returns whether anything was written.
///
/// ⚠⚠ **Only when changed**, and the reason is the sidecar: the caller rewrites `stem.xmp` on
/// `true`, and *"rewriting the same bytes moves the modification time, which is the one value
/// the whole conflict rule turns on"* (`apply_mark_to`). A dialog opened to look and closed
/// unchanged must leave the file alone.
pub fn commit(cat: &Catalog, photo: &Path, draft: &DescriptionDraft) -> bool {
    if !draft.changed() {
        return false;
    }
    cat.set_description(photo, &draft.edited).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn catalogued() -> (Catalog, PathBuf) {
        let cat = Catalog::open_in_memory().unwrap();
        let photo = PathBuf::from("/a/one.RAF");
        cat.sync_folder(
            Path::new("/a"),
            &[catalog::CatalogEntry {
                path: photo.clone(),
                file_name: "one.RAF".into(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        (cat, photo)
    }

    #[test]
    fn opening_reads_what_the_catalog_holds_and_nothing_for_a_fresh_photo() {
        let (cat, photo) = catalogued();
        let fresh = DescriptionDraft::open(Some(&cat), &photo);
        assert_eq!(fresh, DescriptionDraft::default());
        assert!(!fresh.changed());

        cat.set_description(
            &photo,
            &Description {
                title: "Stored".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let d = DescriptionDraft::open(Some(&cat), &photo);
        assert_eq!(d.original.title, "Stored");
        assert_eq!(d.edited, d.original);
        // No catalog at all (single-file mode) is an empty draft, not a panic.
        assert_eq!(DescriptionDraft::open(None, &photo), DescriptionDraft::default());
    }

    #[test]
    fn commit_writes_only_when_the_draft_changed() {
        let (cat, photo) = catalogued();
        let unchanged = DescriptionDraft::open(Some(&cat), &photo);
        assert!(!commit(&cat, &photo, &unchanged), "an untouched draft wrote");
        assert_eq!(cat.description_of(&photo).unwrap(), None);

        let mut d = unchanged;
        d.edited.caption = "Boats returning".into();
        assert!(commit(&cat, &photo, &d));
        assert_eq!(
            cat.description_of(&photo).unwrap().unwrap().caption,
            "Boats returning"
        );
    }

    #[test]
    fn clearing_every_field_commits_as_a_removal() {
        let (cat, photo) = catalogued();
        cat.set_description(
            &photo,
            &Description {
                caption: "old".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut d = DescriptionDraft::open(Some(&cat), &photo);
        d.edited.caption.clear();
        assert!(d.changed());
        assert!(commit(&cat, &photo, &d));
        assert_eq!(cat.description_of(&photo).unwrap(), None);
    }
}
