//! Keeping a photo's `stem.xmp` sidecar and the catalog in step.
//!
//! Ratings, colour labels, keywords and the four description fields are what Rustroom shares
//! with Lightroom.
//! This module owns every rule about *when* and *how*: which files get a sidecar, what a colour
//! label is called in XMP, how a keyword path is created, and the conflict rule.
//!
//! ⚠⚠ **The conflict rule is one comparison and nothing else: a sidecar whose modification time
//! is strictly newer than `photos.xmp_mtime` is a foreign edit and wins.** Our own writes record
//! the file's mtime as they go, so they can never re-import; anything we did not write is newer
//! than what we last saw, and imports. That is why no timestamp is needed on the marks
//! themselves.
//!
//! ⚠ No egui here — this is a lib module, so every rule below is testable without a window.

use catalog::db::{Catalog, Marks};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a photo's XMP sidecar lives: `DSCF1234.xmp` beside `DSCF1234.RAF`.
///
/// ⚠⚠ **`stem.xmp`, not `DSCF1234.RAF.xmp`** — the only name Lightroom looks for, and therefore
/// the only name that delivers the interchange this exists for. ⚠ It is exactly the collision
/// `sidecar::path`'s `.RAF.json` naming was introduced to avoid: a RAW and a JPEG of the same
/// stem would share one `.xmp`. Taken knowingly, and it is why [`takes_sidecar`] is RAW-only.
pub fn sidecar_path(photo: &Path) -> PathBuf {
    photo.with_extension("xmp")
}

/// Whether this file's marks are written to an XMP sidecar at all.
///
/// ⚠ **RAW only.** Lightroom embeds XMP *inside* a JPEG, PNG or TIFF rather than writing a
/// sidecar beside it, and Rustroom never modifies an original — so for those formats there is no
/// convention to match, and writing `photo.xmp` beside `photo.jpg` would be a file no other
/// application reads. Their marks stay catalog-only.
pub fn takes_sidecar(photo: &Path) -> bool {
    catalog::is_raw(photo)
}

/// The catalog's four colour labels and the `xmp:Label` text each one is written as.
///
/// ⚠ Four, not Adobe's five: `Marks::MAX_LABEL` is 4 because keys `6`–`9` are what set a label,
/// and a fifth colour would be unreachable. Adobe's Purple therefore has no Rustroom equivalent
/// and reads as *no label* — see [`label_from_xmp`].
pub const LABELS: [(u8, &str); 4] = [(1, "Red"), (2, "Blue"), (3, "Yellow"), (4, "Green")];

/// What `label` is called in XMP, or `None` for "no label".
pub fn label_to_xmp(label: u8) -> Option<&'static str> {
    LABELS.iter().find(|(n, _)| *n == label).map(|(_, s)| *s)
}

/// Which catalog label `text` names, or `None` if it names none of ours.
///
/// ⚠⚠ **`None` means "leave the catalog's label alone", never "clear it".** Lightroom's Purple,
/// and any custom label text, are labels somebody deliberately set in another application; a
/// rating imported from the same file must not silently discard one. Clearing happens only when
/// the sidecar has no `xmp:Label` at all — see [`apply`].
///
/// ⚠ Case-insensitive: Lightroom writes `Red`, but the file is hand-editable and `red` plainly
/// means the same thing.
pub fn label_from_xmp(text: &str) -> Option<u8> {
    let t = text.trim();
    LABELS
        .iter()
        .find(|(_, s)| s.eq_ignore_ascii_case(t))
        .map(|(n, _)| *n)
}

/// The hierarchical keyword paths assigned to `photo`'s **file**.
///
/// ⚠ Built from `keywords()` plus `keyword_counts()`, which the catalog already has — no new
/// query. ⚠ `keyword_counts` reports **direct** assignments only, which is exactly right here: a
/// photo tagged `Paris` writes the path `Europe|France|Paris`, and writing `Europe` as a second
/// keyword in its own right would claim an assignment nobody made.
///
/// ⚠ Ordered by `keywords()`'s own depth-first, name-ordered traversal, so a sidecar written
/// twice with no change is byte-identical.
pub fn keyword_paths(cat: &Catalog, photo: &Path) -> Vec<Vec<String>> {
    let rows = match cat.keywords() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let assigned = match cat.keyword_counts(&[photo.to_path_buf()]) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let by_id: HashMap<i64, (&str, Option<i64>)> = rows
        .iter()
        .map(|r| (r.id, (r.name.as_str(), r.parent_id)))
        .collect();

    let mut out = Vec::new();
    for row in &rows {
        if !assigned.contains_key(&row.id) {
            continue;
        }
        let mut path = Vec::new();
        let mut at = Some(row.id);
        // ⚠ Bounded by the number of keywords, so a cycle in a hand-edited database cannot spin
        // forever. The schema's foreign key makes one unreachable in practice.
        for _ in 0..=rows.len() {
            let Some(id) = at else { break };
            let Some((name, parent)) = by_id.get(&id) else {
                break;
            };
            path.push((*name).to_string());
            at = *parent;
        }
        path.reverse();
        if !path.is_empty() {
            out.push(path);
        }
    }
    out
}

/// What this photo's sidecar should say, built from the catalog.
///
/// ⚠ **Copy 0 only.** XMP has no notion of a virtual copy, so marks on copies ≥ 1 never reach a
/// sidecar; keywords are per file and shared by every copy anyway.
pub fn sidecar_of(cat: &Catalog, photo: &Path) -> xmp::Sidecar {
    let m = cat.marks(photo, 0).unwrap_or_default();
    let d = cat.description_of(photo).ok().flatten().unwrap_or_default();
    // ⚠ Blank means absent, so a sidecar never carries an empty `dc:title`.
    let field = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    xmp::Sidecar {
        // ⚠ `0` is "unrated", which XMP spells as the property being **absent** rather than as
        // `xmp:Rating` 0 — so an unrated photo's sidecar carries no rating and a reader cannot
        // mistake it for a deliberate zero.
        rating: (m.rating > 0).then_some(m.rating),
        label: label_to_xmp(m.label).map(str::to_string),
        keywords: keyword_paths(cat, photo),
        title: field(&d.title),
        caption: field(&d.caption),
        rights: field(&d.copyright),
        creator: field(&d.creator),
        // ⚠ Never written: a location lives in the catalog only (M10.18b, the author's call).
        gps: None,
    }
}

/// Put the catalog's marks and keywords into `photo`'s sidecar, returning whether one was
/// written.
///
/// ⚠⚠ **Merges into whatever is already there** — see `xmp::write`. A folder edited in Lightroom
/// carries that person's whole develop history in these files.
///
/// ⚠ Every failure is a **skip**, never an error the caller must handle: an unreadable or
/// malformed existing sidecar leaves the file untouched, which is the safe direction, and the
/// catalog is never a reason rating a photo fails. The rule `set_marks` already follows.
pub fn write_for(cat: &Catalog, photo: &Path) -> bool {
    if !takes_sidecar(photo) {
        return false;
    }
    let path = sidecar_path(photo);
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("xmp: cannot read {}: {e}", path.display());
            return false;
        }
    };
    let text = match xmp::write(&sidecar_of(cat, photo), existing.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            // ⚠ Refused rather than replaced. A sidecar we cannot parse is somebody's data.
            eprintln!("xmp: leaving {} alone: {e}", path.display());
            return false;
        }
    };
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("xmp: cannot write {}: {e}", path.display());
        return false;
    }
    // ⚠⚠ **Load-bearing**: without this the file we just wrote is newer than what the catalog
    // has seen, so the next folder open would import our own write straight back — harmless in
    // itself, but it would also overwrite anything the catalog gained since.
    if let Some(secs) = mtime_secs(&path) {
        let _ = cat.set_xmp_mtime(photo, secs);
    }
    true
}

/// Write sidecars for a set of photos, skipping non-RAW files and duplicates.
///
/// ⚠ De-duplicated to **files**: two copies of one photo are one sidecar, and writing it twice
/// would be two identical merges.
pub fn write_for_all(cat: &Catalog, photos: &[PathBuf]) {
    // ⚠ Through `unique::first_of_each`: a whole selection can arrive here (M10.14e).
    for p in crate::unique::first_of_each(photos.iter().map(|p| p.as_path())) {
        write_for(cat, p);
    }
}

/// Put a sidecar's rating, label, keywords and description into the catalog, for copy 0 (the
/// description is per file).
///
/// ⚠ The **flag is preserved**: pick and reject never round-trip in either direction, because
/// Lightroom keeps them catalog-only itself and there is no field to write them to. Importing
/// must therefore not disturb one somebody set here.
pub fn apply(cat: &Catalog, photo: &Path, s: &xmp::Sidecar) {
    // M10.18b: another program's position is a hand-set one — only where the user set nothing, and a
    // sidecar without one leaves the hand row alone (a sidecar cannot say "removed").
    if let Some((la, lo)) = s.gps {
        if cat.hand_location(photo).ok() == Some(catalog::db::Hand::Unset) {
            let _ = cat.set_hand(&[photo.to_path_buf()], catalog::db::Hand::At(la, lo));
        }
    }
    let now = cat.marks(photo, 0).unwrap_or_default();
    let label = match &s.label {
        // ⚠ Unrecognised text keeps the catalog's own label — see `label_from_xmp`.
        Some(t) => label_from_xmp(t).unwrap_or(now.label),
        // No `xmp:Label` at all *is* "no label", and clearing is what makes removing one in
        // Lightroom reach Rustroom.
        None => 0,
    };
    let _ = cat.set_marks(
        photo,
        0,
        Marks {
            rating: s.rating.unwrap_or(0),
            flag: now.flag,
            label,
        },
    );
    // ⚠ Absent means cleared, the rating's rule: `set_description` deletes the row when all
    // four are blank, so a sidecar with no description leaves no row behind.
    let _ = cat.set_description(
        photo,
        &catalog::db::Description {
            title: s.title.clone().unwrap_or_default(),
            caption: s.caption.clone().unwrap_or_default(),
            copyright: s.rights.clone().unwrap_or_default(),
            creator: s.creator.clone().unwrap_or_default(),
        },
    );
    for path in &s.keywords {
        let mut parent: Option<i64> = None;
        let mut leaf: Option<i64> = None;
        for name in path {
            match cat.create_keyword(name, parent) {
                Ok(id) => {
                    parent = Some(id);
                    leaf = Some(id);
                }
                Err(e) => {
                    eprintln!("xmp: cannot create keyword {name}: {e}");
                    leaf = None;
                    break;
                }
            }
        }
        if let Some(id) = leaf {
            let _ = cat.assign_keyword(id, photo);
        }
    }
}

/// Import every sidecar among `photos` that has changed since Rustroom last saw it.
///
/// Returns how many were imported.
///
/// ⚠ One `xmp_mtimes_in` query **per folder**, never per photo — the `marks_in` rule, and this
/// runs on every folder open.
///
/// ⚠ A photo with no catalog row reads as mtime `0`, so its sidecar imports and then
/// `set_xmp_mtime` silently does nothing. Correct: without a row there is nowhere to put the
/// marks either.
pub fn import_newer(cat: &Catalog, photos: &[PathBuf]) -> usize {
    // ⚠ Folders and files through `unique` — a `Vec` searched per photo, twice, made this quadratic:
    // 18.6 s on opening All Photos at 100,000 photos (M10.14e).
    let mut seen: HashMap<PathBuf, i64> = HashMap::new();
    for dir in crate::unique::folders_of(photos.iter().map(|p| p.as_path())) {
        if let Ok(rows) = cat.xmp_mtimes_in(&dir) {
            seen.extend(rows);
        }
    }

    let mut n = 0;
    for photo in crate::unique::first_of_each(photos.iter().map(|p| p.as_path())) {
        if !takes_sidecar(photo) {
            continue;
        }
        let path = sidecar_path(photo);
        let Some(secs) = mtime_secs(&path) else {
            continue;
        };
        // ⚠⚠ **Strictly greater.** A sidecar we wrote ourselves has exactly the recorded time,
        // so it must not re-import; anything a foreign application touched afterwards is later.
        if secs <= seen.get(photo).copied().unwrap_or(0) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match xmp::read(&text) {
            Ok(s) => {
                apply(cat, photo, &s);
                let _ = cat.set_xmp_mtime(photo, secs);
                n += 1;
            }
            // ⚠ Not recorded as seen, so a file repaired by hand imports on the next open.
            Err(e) => eprintln!("xmp: cannot read {}: {e}", path.display()),
        }
    }
    n
}

/// A file's modification time in whole unix seconds, or `None` if it does not exist.
fn mtime_secs(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::db::Flag;

    /// A temp directory that removes itself.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
    /// entering the same tick collide, `cargo test` runs them in parallel, and this project has
    /// paid for that twice.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            Self(test_dir::path(&format!("xmpsync-{tag}")))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A catalogued folder holding one RAW file, and the path to it.
    fn one_raw(tag: &str) -> (TempDir, Catalog, PathBuf) {
        let dir = TempDir::new(tag);
        let photo = dir.0.join("DSCF0001.RAF");
        std::fs::write(&photo, b"not really a raw").unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let scan = catalog::scan_folder(&dir.0).unwrap();
        cat.sync_folder(&dir.0, &scan).unwrap();
        (dir, cat, photo)
    }

    // ─── naming and the label table ───────────────────────────────────────────────────────

    #[test]
    fn the_sidecar_replaces_the_extension_rather_than_appending_to_it() {
        // ⚠ `stem.xmp` is the only name Lightroom looks for. `DSCF0001.RAF.xmp` — the shape
        // `sidecar::path` uses for the JSON — would be invisible to it, which is the whole
        // milestone.
        assert_eq!(
            sidecar_path(Path::new("/x/DSCF0001.RAF")),
            PathBuf::from("/x/DSCF0001.xmp")
        );
    }

    #[test]
    fn only_raw_files_take_a_sidecar() {
        assert!(takes_sidecar(Path::new("/x/a.RAF")));
        assert!(takes_sidecar(Path::new("/x/a.cr3")));
        assert!(!takes_sidecar(Path::new("/x/a.jpg")));
        assert!(!takes_sidecar(Path::new("/x/a.tif")));
    }

    #[test]
    fn the_label_table_is_total_over_every_catalog_label() {
        // ⚠ A label the catalog can hold but the table cannot name would be silently dropped on
        // its way out and never come back — so this sweeps the whole range rather than sampling.
        for n in 1..=Marks::MAX_LABEL {
            let text = label_to_xmp(n).unwrap_or_else(|| panic!("label {n} has no XMP name"));
            assert_eq!(label_from_xmp(text), Some(n), "{text} did not round-trip");
        }
        assert_eq!(label_to_xmp(0), None, "0 is no label, not a colour");
        assert_eq!(label_to_xmp(Marks::MAX_LABEL + 1), None);
    }

    #[test]
    fn an_unrecognised_label_names_none_of_ours() {
        // Adobe's fifth colour, and anything custom.
        assert_eq!(label_from_xmp("Purple"), None);
        assert_eq!(label_from_xmp("To Print"), None);
        // ⚠ Case-insensitive, because the file is hand-editable.
        assert_eq!(label_from_xmp("red"), Some(1));
        assert_eq!(label_from_xmp("  Green  "), Some(4));
    }

    // ─── importing ────────────────────────────────────────────────────────────────────────

    /// M10.18b: another program's position counts as set by hand — but only where nothing was, and a
    /// sidecar without one never removes a hand-set location.
    #[test]
    fn a_sidecars_gps_fills_only_an_unset_hand_row() {
        use catalog::db::Hand;
        let (_d, cat, photo) = one_raw("import-gps");
        let with = |gps| xmp::Sidecar {
            gps,
            ..xmp::Sidecar::default()
        };
        apply(&cat, &photo, &with(Some((38.7, -9.1))));
        assert_eq!(cat.hand_location(&photo).unwrap(), Hand::At(38.7, -9.1));
        apply(&cat, &photo, &with(Some((41.1, -8.6))));
        assert_eq!(
            cat.hand_location(&photo).unwrap(),
            Hand::At(38.7, -9.1),
            "a sidecar overwrote a hand row"
        );
        cat.set_hand(&[photo.clone()], Hand::Removed).unwrap();
        apply(&cat, &photo, &with(None));
        assert_eq!(cat.hand_location(&photo).unwrap(), Hand::Removed);
    }

    #[test]
    fn importing_sets_the_rating_and_the_label() {
        let (_d, cat, photo) = one_raw("import");
        apply(
            &cat,
            &photo,
            &xmp::Sidecar {
                rating: Some(4),
                label: Some("Blue".into()),
                keywords: Vec::new(),
                ..xmp::Sidecar::default()
            },
        );
        let m = cat.marks(&photo, 0).unwrap();
        assert_eq!((m.rating, m.label), (4, 2));
    }

    #[test]
    fn a_label_outside_the_four_leaves_the_catalog_value_alone() {
        // ⚠⚠ The discriminating case: Purple is a label somebody set deliberately in another
        // application, and importing a *rating* from the same file must not discard it.
        let (_d, cat, photo) = one_raw("purple");
        cat.set_marks(
            &photo,
            0,
            Marks {
                label: 3,
                ..Marks::default()
            },
        )
        .unwrap();
        apply(
            &cat,
            &photo,
            &xmp::Sidecar {
                rating: Some(2),
                label: Some("Purple".into()),
                keywords: Vec::new(),
                ..xmp::Sidecar::default()
            },
        );
        let m = cat.marks(&photo, 0).unwrap();
        assert_eq!(m.label, 3, "an unknown label overwrote ours");
        assert_eq!(m.rating, 2, "the rating still imported");
    }

    #[test]
    fn an_absent_label_clears_and_an_absent_rating_is_unrated() {
        // ⚠ Distinct from the test above: *no* `xmp:Label` means the label was removed
        // elsewhere, which must reach us — otherwise clearing a label in Lightroom is invisible.
        let (_d, cat, photo) = one_raw("clear");
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 5,
                label: 1,
                ..Marks::default()
            },
        )
        .unwrap();
        apply(&cat, &photo, &xmp::Sidecar::default());
        let m = cat.marks(&photo, 0).unwrap();
        assert_eq!((m.rating, m.label), (0, 0));
    }

    #[test]
    fn the_flag_survives_an_import() {
        // ⚠ Flags never round-trip in either direction — Lightroom keeps them catalog-only too —
        // so an import must leave one alone rather than clearing it as "absent".
        let (_d, cat, photo) = one_raw("flag");
        cat.set_marks(
            &photo,
            0,
            Marks {
                flag: Flag::Pick,
                ..Marks::default()
            },
        )
        .unwrap();
        apply(
            &cat,
            &photo,
            &xmp::Sidecar {
                rating: Some(3),
                ..xmp::Sidecar::default()
            },
        );
        assert_eq!(cat.marks(&photo, 0).unwrap().flag, Flag::Pick);
    }

    #[test]
    fn importing_creates_the_keyword_path_and_assigns_only_the_leaf() {
        let (_d, cat, photo) = one_raw("kwpath");
        apply(
            &cat,
            &photo,
            &xmp::Sidecar {
                keywords: vec![vec!["Europe".into(), "France".into(), "Paris".into()]],
                ..xmp::Sidecar::default()
            },
        );
        let rows = cat.keywords().unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"Europe") && names.contains(&"France") && names.contains(&"Paris"),
            "the path was not created: {names:?}"
        );
        // ⚠ Only the leaf is *assigned*; the ancestors exist so the tree has a shape, and
        // `keyword_members` finds the photo under Europe by expanding descendants.
        let counts = cat.keyword_counts(std::slice::from_ref(&photo)).unwrap();
        let paris = rows.iter().find(|r| r.name == "Paris").unwrap();
        let europe = rows.iter().find(|r| r.name == "Europe").unwrap();
        assert_eq!(counts.get(&paris.id), Some(&1));
        assert_eq!(counts.get(&europe.id), None, "an ancestor was assigned");
        // And the photo is found by browsing the root, which is what the hierarchy is for.
        assert_eq!(cat.keyword_members(europe.id).unwrap().len(), 1);
    }

    #[test]
    fn keyword_paths_come_back_as_the_paths_that_went_in() {
        let (_d, cat, photo) = one_raw("kwround");
        let want = vec![
            vec!["Europe".to_string(), "France".into(), "Paris".into()],
            vec!["Street".to_string()],
        ];
        apply(
            &cat,
            &photo,
            &xmp::Sidecar {
                keywords: want.clone(),
                ..xmp::Sidecar::default()
            },
        );
        let mut got = keyword_paths(&cat, &photo);
        got.sort();
        let mut want = want;
        want.sort();
        assert_eq!(got, want);
    }

    // ─── writing ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn writing_then_reading_round_trips_through_the_catalog() {
        let (_d, cat, photo) = one_raw("roundtrip");
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 3,
                label: 4,
                ..Marks::default()
            },
        )
        .unwrap();
        let k = cat.create_keyword("Europe", None).unwrap();
        let k2 = cat.create_keyword("Paris", Some(k)).unwrap();
        cat.assign_keyword(k2, &photo).unwrap();

        assert!(write_for(&cat, &photo), "nothing was written");
        let text = std::fs::read_to_string(sidecar_path(&photo)).unwrap();
        let back = xmp::read(&text).unwrap();
        assert_eq!(back.rating, Some(3));
        assert_eq!(back.label.as_deref(), Some("Green"));
        assert_eq!(
            back.keywords,
            vec![vec!["Europe".to_string(), "Paris".to_string()]]
        );
    }

    #[test]
    fn a_description_reaches_the_sidecar_and_comes_back() {
        let (_d, cat, photo) = one_raw("desc");
        let d = catalog::db::Description {
            title: "Harbour".into(),
            caption: "Boats".into(),
            copyright: "© W".into(),
            creator: "W".into(),
        };
        cat.set_description(&photo, &d).unwrap();
        assert!(write_for(&cat, &photo));
        let text = std::fs::read_to_string(sidecar_path(&photo)).unwrap();
        let back = xmp::read(&text).unwrap();
        assert_eq!(back.title.as_deref(), Some("Harbour"));
        assert_eq!(back.caption.as_deref(), Some("Boats"));
        assert_eq!(back.rights.as_deref(), Some("© W"));
        assert_eq!(back.creator.as_deref(), Some("W"));

        // And the other direction, from a fresh catalog.
        let (_d2, cat2, photo2) = one_raw("desc_back");
        apply(&cat2, &photo2, &back);
        assert_eq!(cat2.description_of(&photo2).unwrap(), Some(d));
    }

    #[test]
    fn an_absent_description_field_clears_the_catalogs() {
        // ⚠ The rating's rule, one field over: *no* `dc:description` in a newer sidecar means
        // the caption was removed elsewhere, and that must reach us.
        let (_d, cat, photo) = one_raw("desc_clear");
        cat.set_description(
            &photo,
            &catalog::db::Description {
                caption: "stale".into(),
                ..Default::default()
            },
        )
        .unwrap();
        apply(&cat, &photo, &xmp::Sidecar::default());
        assert_eq!(cat.description_of(&photo).unwrap(), None);
    }

    #[test]
    fn a_jpeg_never_gets_a_sidecar() {
        let dir = TempDir::new("jpeg");
        let photo = dir.0.join("a.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let scan = catalog::scan_folder(&dir.0).unwrap();
        cat.sync_folder(&dir.0, &scan).unwrap();
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 5,
                ..Marks::default()
            },
        )
        .unwrap();

        assert!(!write_for(&cat, &photo));
        assert!(
            !sidecar_path(&photo).exists(),
            "a JPEG got an .xmp beside it"
        );
    }

    #[test]
    fn writing_merges_into_a_sidecar_somebody_else_wrote() {
        // ⚠⚠ The app-level half of the load-bearing test: a real file on disk, rewritten in
        // place, with every `crs:` line still present afterwards.
        let (_d, cat, photo) = one_raw("merge");
        let foreign = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="1"
   crs:Exposure2012="+0.75"
   crs:Contrast2012="+20"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        std::fs::write(sidecar_path(&photo), foreign).unwrap();
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 5,
                ..Marks::default()
            },
        )
        .unwrap();

        assert!(write_for(&cat, &photo));
        let after = std::fs::read_to_string(sidecar_path(&photo)).unwrap();
        // ⚠ Compared as **attributes**, not as whole lines: this fixture's `rdf:Description` is
        // self-closing, so the merge has to *open* it to hold our children and the last line
        // legitimately goes from `"+20"/>` to `"+20">`. A first draft asserted on lines and was
        // simply wrong about correct code.
        for attr in [
            r#"xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/""#,
            r#"crs:Exposure2012="+0.75""#,
            r#"crs:Contrast2012="+20""#,
        ] {
            assert!(
                after.contains(attr),
                "a develop setting was lost: {attr}\n--- after ---\n{after}"
            );
        }
        assert_eq!(xmp::read(&after).unwrap().rating, Some(5));
    }

    // ─── the conflict rule ────────────────────────────────────────────────────────────────

    /// Write a sidecar and stamp it `secs` seconds in the future or past.
    fn put_sidecar(photo: &Path, rating: u8, offset: i64) {
        let text = xmp::write(
            &xmp::Sidecar {
                rating: Some(rating),
                ..xmp::Sidecar::default()
            },
            None,
        )
        .unwrap();
        let path = sidecar_path(photo);
        std::fs::write(&path, text).unwrap();
        let now = std::time::SystemTime::now();
        let when = if offset >= 0 {
            now + std::time::Duration::from_secs(offset as u64)
        } else {
            now - std::time::Duration::from_secs((-offset) as u64)
        };
        filetime::set_mtime(&path, when);
    }

    /// The one bit of `filetime` this module's tests need, without a dependency.
    mod filetime {
        use std::path::Path;
        use std::time::SystemTime;

        /// ⚠ Written through an `OpenOptions` round trip rather than a crate: the tests only
        /// need the mtime to move, and `File::set_times` is stable.
        pub fn set_mtime(path: &Path, when: SystemTime) {
            let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(when))
                .unwrap();
        }
    }

    #[test]
    fn a_newer_sidecar_imports_and_an_older_one_does_not() {
        let (_d, cat, photo) = one_raw("newer");
        // The catalog last saw this sidecar "now"; the file on disk is older than that.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        put_sidecar(&photo, 2, -60);
        cat.set_xmp_mtime(&photo, now).unwrap();

        assert_eq!(
            import_newer(&cat, std::slice::from_ref(&photo)),
            0,
            "an older sidecar was imported"
        );
        assert_eq!(cat.marks(&photo, 0).unwrap().rating, 0);

        // Now somebody edits it in Lightroom, so it becomes newer than what we last saw.
        put_sidecar(&photo, 4, 60);
        assert_eq!(import_newer(&cat, std::slice::from_ref(&photo)), 1);
        assert_eq!(cat.marks(&photo, 0).unwrap().rating, 4);
    }

    #[test]
    fn a_sidecar_the_catalog_has_never_seen_imports() {
        // ⚠ The case that matters most: a folder rated in Lightroom, opened here for the first
        // time. Its rows carry `xmp_mtime` 0, so every sidecar is newer.
        let (_d, cat, photo) = one_raw("firstopen");
        put_sidecar(&photo, 5, 0);
        assert_eq!(import_newer(&cat, std::slice::from_ref(&photo)), 1);
        assert_eq!(cat.marks(&photo, 0).unwrap().rating, 5);
    }

    #[test]
    fn our_own_write_does_not_re_import() {
        // ⚠⚠ The other half of the conflict rule, and the one a "does a newer file import?" test
        // structurally cannot see: writing records the file's own mtime, so the very next open
        // must find nothing to do. Without that, every rating would bounce back through the
        // sidecar on the next folder open.
        let (_d, cat, photo) = one_raw("noreimport");
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 4,
                ..Marks::default()
            },
        )
        .unwrap();
        assert!(write_for(&cat, &photo));

        assert_eq!(
            import_newer(&cat, std::slice::from_ref(&photo)),
            0,
            "our own write imported itself back"
        );
    }

    #[test]
    fn a_photo_with_no_sidecar_imports_nothing() {
        let (_d, cat, photo) = one_raw("absent");
        assert_eq!(import_newer(&cat, &[photo]), 0);
    }

    #[test]
    fn a_malformed_sidecar_is_skipped_rather_than_importing_or_panicking() {
        let (_d, cat, photo) = one_raw("malformed");
        std::fs::write(sidecar_path(&photo), "<a><b></a>").unwrap();
        assert_eq!(import_newer(&cat, std::slice::from_ref(&photo)), 0);
        assert_eq!(cat.marks(&photo, 0).unwrap(), Marks::default());
        // ⚠ And it is **not** recorded as seen, so repairing the file by hand imports it on the
        // next open rather than needing the mtime bumped as well.
        assert_eq!(cat.xmp_mtimes_in(photo.parent().unwrap()).unwrap()[0].1, 0);
    }

    #[test]
    fn a_malformed_existing_sidecar_is_never_overwritten() {
        // ⚠ The safe direction: somebody's unparseable file is still somebody's file, and
        // replacing it with a fresh packet holding a rating is the destruction the merge exists
        // to prevent.
        let (_d, cat, photo) = one_raw("keepbad");
        std::fs::write(sidecar_path(&photo), "<a><b></a>").unwrap();
        cat.set_marks(
            &photo,
            0,
            Marks {
                rating: 5,
                ..Marks::default()
            },
        )
        .unwrap();
        assert!(!write_for(&cat, &photo));
        assert_eq!(
            std::fs::read_to_string(sidecar_path(&photo)).unwrap(),
            "<a><b></a>"
        );
    }
}
