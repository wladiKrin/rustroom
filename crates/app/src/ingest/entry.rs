//! Applying the dialog's metadata and develop preset to a photo as it arrives (M9.12's "on entry").
//!
//! ⚠ The photo's folder must already be in the catalog (`sync_folder`) — every catalog write here is
//! a quiet no-op otherwise, `set_description`'s own rule. The flow scans the destination first.
//!
//! ⚠ XMP is NOT written here: the flow calls `write_xmp_for` once for every imported file, the one
//! funnel that honours `Settings.write_xmp` and the RAW-only rule.

use super::options::ImportOptions;
use crate::photo_id::PhotoId;
use crate::presets::Preset;
use catalog::db::{Catalog, Description};
use core_image::Capture;
use std::path::Path;

/// Record `capture`, merge the non-empty Copyright / Creator, assign the keywords and apply the
/// preset — for one imported photo.
pub fn apply(
    cat: &Catalog,
    photo: &Path,
    capture: &Capture,
    opts: &ImportOptions,
    preset: Option<&Preset>,
) {
    // ⚠ At once, so the NEXT import's already-imported check sees this photo without waiting for
    // the background metadata scan to reach it.
    let _ = cat.set_capture(photo, capture);
    // M10.18b: and where it was taken — an import and a tethered shot both land here.
    let _ = cat.set_file_location(photo, capture.gps);

    // ⚠⚠ Only the fields the dialog fills: an empty Creator must never erase one already there.
    let (copyright, creator) = (opts.copyright.trim(), opts.creator.trim());
    if !copyright.is_empty() || !creator.is_empty() {
        let mut d: Description = cat.description_of(photo).ok().flatten().unwrap_or_default();
        if !copyright.is_empty() {
            d.copyright = copyright.to_string();
        }
        if !creator.is_empty() {
            d.creator = creator.to_string();
        }
        let _ = cat.set_description(photo, &d);
    }

    for name in opts.keyword_list() {
        if let Ok(id) = cat.create_keyword(&name, None) {
            let _ = cat.assign_keyword(id, photo);
        }
    }

    if let Some(p) = preset {
        crate::sync::apply(
            &[PhotoId::original(photo.to_path_buf())],
            &p.edits,
            p.groups,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::temp_dir;
    use catalog::CatalogEntry;
    use edit_model::{EditGroups, Edits};

    /// A catalog holding one real file, as the flow leaves it after scanning the destination.
    fn catalogued(tag: &str) -> (Catalog, std::path::PathBuf) {
        let d = temp_dir(tag);
        let photo = d.join("DSCF0001.RAF");
        std::fs::write(&photo, b"raw").unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            &d,
            &[CatalogEntry {
                path: photo.clone(),
                file_name: "DSCF0001.RAF".into(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        (cat, photo)
    }

    fn opts() -> ImportOptions {
        ImportOptions {
            copyright: "© 2026 W".into(),
            creator: "W".into(),
            keywords: "Paris, family".into(),
            ..ImportOptions::default()
        }
    }

    /// M10.18b: an imported (or tethered) photo records where it was taken at once.
    #[test]
    fn a_scanned_photo_records_its_file_location() {
        let (cat, photo) = catalogued("location");
        let capture = Capture {
            gps: Some((38.7, -9.1)),
            ..Capture::default()
        };
        apply(&cat, &photo, &capture, &ImportOptions::default(), None);
        assert_eq!(cat.location_of(&photo).unwrap(), Some((38.7, -9.1)));
        assert!(cat.files_without_location_read().unwrap().is_empty());
    }

    #[test]
    fn copyright_creator_keywords_and_capture_reach_the_catalog() {
        let (cat, photo) = catalogued("meta");
        let cap = Capture {
            taken: Some(42),
            ..Default::default()
        };
        apply(&cat, &photo, &cap, &opts(), None);
        let d = cat.description_of(&photo).unwrap().unwrap();
        assert_eq!(
            (d.copyright.as_str(), d.creator.as_str()),
            ("© 2026 W", "W")
        );
        let names: Vec<String> = cat
            .keywords()
            .unwrap()
            .into_iter()
            .map(|k| k.name)
            .collect();
        assert_eq!(names, ["family", "Paris"]);
        let counts: Vec<i64> = cat
            .keywords()
            .unwrap()
            .into_iter()
            .map(|k| k.count)
            .collect();
        assert_eq!(counts, [1, 1], "each keyword assigned to the photo");
        assert_eq!(cat.import_fingerprints().unwrap()[0].taken, Some(42));
        // A second import of the same keyword does not duplicate it.
        apply(&cat, &photo, &cap, &opts(), None);
        assert_eq!(cat.keywords().unwrap().len(), 2);
    }

    #[test]
    fn an_empty_field_erases_nothing() {
        let (cat, photo) = catalogued("keep");
        cat.set_description(
            &photo,
            &Description {
                creator: "Earlier".into(),
                title: "Title".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let only_copyright = ImportOptions {
            copyright: "© X".into(),
            ..ImportOptions::default()
        };
        apply(&cat, &photo, &Capture::default(), &only_copyright, None);
        let d = cat.description_of(&photo).unwrap().unwrap();
        assert_eq!(d.creator, "Earlier");
        assert_eq!(d.title, "Title");
        assert_eq!(d.copyright, "© X");
    }

    #[test]
    fn a_preset_reaches_the_sidecar_and_none_writes_none() {
        let (cat, photo) = catalogued("preset");
        apply(
            &cat,
            &photo,
            &Capture::default(),
            &ImportOptions::default(),
            None,
        );
        assert!(
            !crate::sidecar::path(&photo).exists(),
            "no preset must write no sidecar"
        );
        let preset = Preset {
            name: "Bright".into(),
            groups: EditGroups::ALL,
            edits: Edits {
                exposure: 0.7,
                ..Edits::default()
            },
            folder: None,
        };
        apply(
            &cat,
            &photo,
            &Capture::default(),
            &ImportOptions::default(),
            Some(&preset),
        );
        let side = crate::sidecar::load_all(&photo);
        assert_eq!(side.get(0).unwrap().edits.exposure, 0.7);
    }
}
