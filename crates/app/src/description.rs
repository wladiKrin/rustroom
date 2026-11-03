//! The Information dialog's editable description: a draft, and the one rule about committing it.
//!
//! ⚠ No egui here — a lib module, so the commit rule is testable without a window. The dialog
//! (`modals/info.rs`) owns the fields; this owns *when a write happens*.
//!
//! [`BatchDraft`] is the same for several photos at once (`modals/describe_many.rs`): a field shows
//! the value they share or reads as mixed, and only a field the user typed in is written to them all.

use catalog::db::{Catalog, Description};
use std::path::{Path, PathBuf};

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

/// One of a description's four fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Caption,
    Copyright,
    Creator,
}

impl Field {
    pub const ALL: [Field; 4] = [
        Field::Title,
        Field::Caption,
        Field::Copyright,
        Field::Creator,
    ];

    fn of(self, d: &Description) -> &str {
        match self {
            Field::Title => &d.title,
            Field::Caption => &d.caption,
            Field::Copyright => &d.copyright,
            Field::Creator => &d.creator,
        }
    }

    fn of_mut(self, d: &mut Description) -> &mut String {
        match self {
            Field::Title => &mut d.title,
            Field::Caption => &mut d.caption,
            Field::Copyright => &mut d.copyright,
            Field::Creator => &mut d.creator,
        }
    }
}

/// Several photos' description, edited at once. Indexed by `Field as usize`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchDraft {
    /// Each file once — copies share a file, and so its description.
    pub photos: Vec<PathBuf>,
    /// The value every photo holds, or `None` where they differ (mixed).
    shared: [Option<String>; 4],
    /// What the field holds now — the shared value, or blank when mixed.
    pub edited: [String; 4],
    /// Whether the user typed in the field. ⚠ Only these are written: a mixed field left alone keeps
    /// each photo's own value.
    pub touched: [bool; 4],
}

impl BatchDraft {
    /// Read every photo's description once, for the dialog to edit.
    pub fn open(cat: Option<&Catalog>, photos: &[PathBuf]) -> Self {
        let photos = crate::unique::first_of_each(photos.iter().cloned());
        let read: Vec<Description> = photos
            .iter()
            .map(|p| {
                cat.and_then(|c| c.description_of(p).ok().flatten())
                    .unwrap_or_default()
            })
            .collect();
        let shared = Field::ALL.map(|f| {
            let first = read.first().map_or("", |d| f.of(d));
            read.iter()
                .all(|d| f.of(d) == first)
                .then(|| first.to_string())
        });
        Self {
            photos,
            edited: shared.clone().map(Option::unwrap_or_default),
            shared,
            touched: [false; 4],
        }
    }

    /// The value every photo holds in `f`, or `None` when it is mixed.
    pub fn shared(&self, f: Field) -> Option<&str> {
        self.shared[f as usize].as_deref()
    }

    /// Type `text` into `f`.
    pub fn set(&mut self, f: Field, text: &str) {
        self.edited[f as usize] = text.to_string();
        self.touched[f as usize] = true;
    }
}

/// Write the draft's touched fields to every photo. Returns the photos whose description changed —
/// the caller rewrites their sidecars, and only theirs, for `commit`'s reason.
pub fn commit_many(cat: &Catalog, draft: &BatchDraft) -> Vec<PathBuf> {
    let mut written = Vec::new();
    for p in &draft.photos {
        let before = cat.description_of(p).ok().flatten().unwrap_or_default();
        let mut d = before.clone();
        for f in Field::ALL {
            if draft.touched[f as usize] {
                *f.of_mut(&mut d) = draft.edited[f as usize].clone();
            }
        }
        if d != before && cat.set_description(p, &d).is_ok() {
            written.push(p.clone());
        }
    }
    written
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
        assert_eq!(
            DescriptionDraft::open(None, &photo),
            DescriptionDraft::default()
        );
    }

    #[test]
    fn commit_writes_only_when_the_draft_changed() {
        let (cat, photo) = catalogued();
        let unchanged = DescriptionDraft::open(Some(&cat), &photo);
        assert!(
            !commit(&cat, &photo, &unchanged),
            "an untouched draft wrote"
        );
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

    fn three_catalogued() -> (Catalog, Vec<PathBuf>) {
        let cat = Catalog::open_in_memory().unwrap();
        let photos: Vec<PathBuf> = ["one", "two", "three"]
            .iter()
            .map(|n| PathBuf::from(format!("/a/{n}.RAF")))
            .collect();
        let entries: Vec<_> = photos
            .iter()
            .map(|p| catalog::CatalogEntry {
                path: p.clone(),
                file_name: p.file_name().unwrap().to_string_lossy().into(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        cat.sync_folder(Path::new("/a"), &entries).unwrap();
        (cat, photos)
    }

    fn described(title: &str, caption: &str) -> Description {
        Description {
            title: title.into(),
            caption: caption.into(),
            ..Default::default()
        }
    }

    #[test]
    fn several_photos_show_a_shared_value_and_mixed_where_they_differ() {
        let (cat, photos) = three_catalogued();
        for p in &photos {
            cat.set_description(p, &described("Harbour", "")).unwrap();
        }
        cat.set_description(&photos[2], &described("Harbour", "Boats"))
            .unwrap();
        let d = BatchDraft::open(Some(&cat), &photos);
        assert_eq!(d.shared(Field::Title), Some("Harbour"));
        assert_eq!(d.edited[Field::Title as usize], "Harbour");
        assert_eq!(
            d.shared(Field::Caption),
            None,
            "two blank and one 'Boats' is mixed"
        );
        assert_eq!(d.edited[Field::Caption as usize], "");
        assert_eq!(
            d.shared(Field::Creator),
            Some(""),
            "all blank is shared, not mixed"
        );
    }

    #[test]
    fn only_a_touched_field_is_written_to_every_photo() {
        let (cat, photos) = three_catalogued();
        cat.set_description(&photos[0], &described("First", ""))
            .unwrap();
        cat.set_description(&photos[1], &described("Second", ""))
            .unwrap();
        let mut d = BatchDraft::open(Some(&cat), &photos);
        d.set(Field::Caption, "Boats returning");
        let written = commit_many(&cat, &d);
        assert_eq!(written, photos, "every photo's caption changed");
        let titles: Vec<String> = photos
            .iter()
            .map(|p| cat.description_of(p).unwrap().unwrap_or_default().title)
            .collect();
        assert_eq!(
            titles,
            ["First", "Second", ""],
            "the untouched mixed title was kept"
        );
        for p in &photos {
            assert_eq!(
                cat.description_of(p).unwrap().unwrap().caption,
                "Boats returning"
            );
        }
    }

    #[test]
    fn an_untouched_draft_writes_nothing_and_a_cleared_field_clears_it_everywhere() {
        let (cat, photos) = three_catalogued();
        cat.set_description(&photos[0], &described("", "old"))
            .unwrap();
        cat.set_description(&photos[1], &described("", "other"))
            .unwrap();
        let d = BatchDraft::open(Some(&cat), &photos);
        assert!(commit_many(&cat, &d).is_empty(), "an untouched draft wrote");
        let mut d = d;
        d.set(Field::Caption, "");
        assert_eq!(
            commit_many(&cat, &d),
            photos[..2].to_vec(),
            "only the two that held a caption changed"
        );
        assert_eq!(cat.description_of(&photos[0]).unwrap(), None);
        assert_eq!(cat.description_of(&photos[1]).unwrap(), None);
    }

    #[test]
    fn a_file_listed_twice_is_one_photo() {
        let (cat, photos) = three_catalogued();
        let twice = vec![photos[0].clone(), photos[0].clone()];
        let mut d = BatchDraft::open(Some(&cat), &twice);
        assert_eq!(d.photos, vec![photos[0].clone()]);
        d.set(Field::Title, "T");
        assert_eq!(commit_many(&cat, &d), vec![photos[0].clone()]);
    }
}
