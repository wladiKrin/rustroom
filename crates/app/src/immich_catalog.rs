//! Immich assets as catalog rows, so a mark set on one is still there tomorrow.
//!
//! ⚠⚠ **Why this exists.** `Catalog::set_marks` opens with `let Some(id) = self.photo_id(..)? else {
//! return Ok(()) }` — with no `photos` row it returns `Ok(())` and does **nothing**. The Immich
//! openers never called `sync_folder`, so every rating, flag, label and keyword set on an Immich photo
//! was silently dropped, and nothing failed at the time. `test_app::catalog_library`'s own doc had
//! already written that trap down.
//!
//! ⚠ **Local only, by decision.** Nothing here talks to Immich. Writing marks back would need
//! `asset.update` on the API key, which the Settings help text tells the user not to grant.
//!
//! ⚠ **No egui.** A lib module, testable with no window and no server.

use std::path::Path;

/// Record an opened album's or month's assets, so their marks have somewhere to live.
///
/// ⚠⚠ **The folder is the server's cache root — one row per server, never one per album.** An asset
/// reached through two albums, or through an album and the timeline, is **one photo with one
/// identity**; a folder per album would give it two rows and two independent sets of marks.
///
/// ⚠ Rows accumulate and are never removed: `sync_folder`'s documented rule is that *absence is not
/// deletion*, which is exactly what makes a mark on an asset you have not looked at for a month still
/// be there. Calling this for a second album adds to the first rather than replacing it.
///
/// ⚠ An asset that has not been downloaded still gets a usable row — `file_facts` records a `None`
/// size for a missing file — because marking a photo straight from a server thumbnail has to work.
///
/// ⚠ Also records each download's original name (`set_original_names`), which Move to folder gives
/// the file when it leaves the cache.
pub fn record_assets(
    cat: &catalog::db::Catalog,
    root: &Path,
    scan: &[catalog::CatalogEntry],
) -> Result<(), catalog::db::CatalogError> {
    cat.sync_folder(root, scan)?;
    // ⚠ The name each download had on the phone — `CatalogEntry::file_name` here, the asset's
    // `original_file_name` — kept for Move to folder, since the cache names it by asset id.
    let names: Vec<(String, String)> = scan
        .iter()
        .filter_map(|e| {
            let own = e.path.file_name()?.to_string_lossy().into_owned();
            Some((own, e.file_name.clone()))
        })
        .collect();
    cat.set_original_names(root, &names)
}

/// What Immich's own marks become in Rustroom (M10.15c, the author's mapping): 1–5 stars; a
/// favourite → pick; −1 (rejected) → reject and no stars; a favourite rejected → pick, the heart
/// being the more deliberate mark. Anything else is nothing.
///
/// ⚠ −1 is handled although the reply's schema says 1–5: writing a rating allows −1, and no real
/// reply has settled what the server sends back.
pub fn server_marks(a: &immich::Asset) -> (Option<u8>, Option<catalog::db::Flag>) {
    use catalog::db::Flag;
    let rating = a.exif_info.as_ref().and_then(|e| e.rating);
    let stars = rating.filter(|r| (1..=5).contains(r)).map(|r| r as u8);
    let flag = if a.is_favorite {
        Some(Flag::Pick)
    } else if rating == Some(-1) {
        Some(Flag::Reject)
    } else {
        None
    };
    (stars, flag)
}

/// Fill the catalog's marks of `assets` (cached under `root`) from Immich's own — once each, never
/// over the user's (`Catalog::fill_from_server` holds that rule). Only assets carrying a mark are
/// passed on, so a list with none costs no query at all.
pub fn fill_marks(
    cat: &catalog::db::Catalog,
    root: &Path,
    assets: &[immich::Asset],
) -> Result<usize, catalog::db::CatalogError> {
    let fills: Vec<catalog::db::ServerFill> = assets
        .iter()
        .filter_map(|a| {
            let (rating, flag) = server_marks(a);
            (rating.is_some() || flag.is_some()).then(|| catalog::db::ServerFill {
                photo: crate::immich_source::cache_path(root, a),
                rating,
                flag,
            })
        })
        .collect();
    if fills.is_empty() {
        return Ok(0);
    }
    cat.fill_from_server(&fills)
}

/// Record Immich's positions for `assets` (cached under `root`) as their `server` location rows
/// (M10.18b). Only assets with both halves are passed on; the rule that a hand-set or the file's own
/// position wins is `Catalog::location_of`'s.
pub fn fill_locations(
    cat: &catalog::db::Catalog,
    root: &Path,
    assets: &[immich::Asset],
) -> Result<usize, catalog::db::CatalogError> {
    let rows: Vec<(std::path::PathBuf, (f64, f64))> = assets
        .iter()
        .filter_map(|a| {
            let e = a.exif_info.as_ref()?;
            let at = (e.latitude?, e.longitude?);
            let ok = at.0.abs() <= 90.0 && at.1.abs() <= 180.0 && at != (0.0, 0.0);
            ok.then(|| (crate::immich_source::cache_path(root, a), at))
        })
        .collect();
    if rows.is_empty() {
        return Ok(0);
    }
    cat.set_server_locations(&rows)
}

/// Immich's people in `assets` (cached under `root`) and their face boxes (M10.18c) — the named, visible ones;
/// each asset's earlier Immich faces replaced. Returns how many people were seen.
pub fn fill_faces(
    cat: &catalog::db::Catalog,
    root: &Path,
    assets: &[immich::Asset],
) -> Result<usize, catalog::db::CatalogError> {
    let named = |p: &&immich::AssetPerson| !p.is_hidden && !p.name.trim().is_empty();
    let mut people: Vec<(String, String)> = assets
        .iter()
        .flat_map(|a| a.people.iter().flatten().filter(named))
        .map(|p| (p.id.clone(), p.name.trim().to_string()))
        .collect();
    people.sort();
    people.dedup();
    if !people.is_empty() {
        cat.set_immich_people(&people)?;
    }
    // ⚠ Only assets the reply spoke for (`Some`, even empty); one transaction for them all (the review's #10).
    let faces: Vec<catalog::db::ImmichFaces> = assets
        .iter()
        .filter_map(|a| {
            let list = a.people.as_ref()?;
            let boxes = list
                .iter()
                .filter(named)
                .flat_map(|p| {
                    p.faces
                        .iter()
                        .filter_map(|f| f.normalised())
                        .map(|b| (b, p.id.clone()))
                })
                .collect();
            Some((crate::immich_source::cache_path(root, a), boxes))
        })
        .collect();
    if !faces.is_empty() {
        cat.set_immich_faces(&faces)?;
    }
    Ok(people.len())
}

/// Whether this grid cell should carry the downloaded indicator.
///
/// ⚠ **Immich sources only** — asked for under the Immich header and nowhere else. A local photo is
/// always "downloaded", so the mark would be noise on every cell of every folder.
///
/// ⚠ Answered from the pre-read `present` set, never a stat: the grid draws hundreds of cells sixty
/// times a second. See `immich_source::present_names`.
pub fn shows_download_badge(
    is_immich_source: bool,
    path: &Path,
    present: &std::collections::HashSet<std::ffi::OsString>,
) -> bool {
    is_immich_source && path.file_name().is_some_and(|n| present.contains(n))
}

/// What the Folders rail should call this folder.
///
/// ⚠ The Immich cache root's basename is the host slug — `https___immich_example` — which
/// says nothing to anybody. **Renamed rather than hidden** (user's call): clicking it opens the cache
/// directory as an ordinary folder, which shows exactly the photos that have been downloaded, so the
/// name is honest rather than a euphemism. The full path is still the row's hover.
///
/// ⚠ Compared against the **configured** root, not against the shape of the name: a folder that merely
/// looks like a host slug keeps its own name.
pub fn folder_display_name(path: &Path, immich_root: Option<&Path>) -> String {
    if immich_root == Some(path) {
        // ⚠⚠ **The name is the same whether or not the integration is switched on** (user's call,
        // 2026-09-24, revising their own "no mention of immich at all"). A neutral "Downloaded
        // photos" was built first and taken back out: these are the user's photographs and they
        // know where they came from, so a name that hides it helps nobody.
        //
        // ⚠ Which also means the row's hover stays the real path — the `folder_hover` helper that
        // existed to blank it is gone.
        return "immich (downloaded)".to_string();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Whether a catalog row should appear in a **catalog-wide** view — All Photos, or a smart collection.
///
/// ⚠⚠ **Immich rows only, and that restriction is the point.** A local folder on an unplugged drive
/// also holds rows whose files are absent, and the catalog keeps them deliberately. Hiding every absent
/// row would quietly change what an unplugged drive does, which is a different question that nobody
/// asked.
///
/// ⚠ An Immich row is shown only once its file is really on disk — the user's call, and the same line
/// he drew for slice 2: a photo downloaded from Immich becomes local and joins everything else.
///
/// ⚠ One `exists` per Immich row, on an explicit action rather than per frame. It stats **live**, which
/// is what makes a just-downloaded photo appear without anything re-syncing its row. If a library of
/// 100k browsed assets ever makes the cost felt, the fix is a `present` column maintained at download
/// time, not a cache in front of the stat.
pub fn row_is_browsable(path: &Path, immich_root: Option<&Path>) -> bool {
    match immich_root {
        Some(root) if crate::immich_source::is_immich_path(root, path) => path.exists(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory of its own — `test_dir` makes it unique per test and per run.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        test_dir::path(&format!("imcat-{tag}"))
    }

    /// ⚠⚠ **The asset's id and its original file name must DIFFER, and this fixture once made them
    /// the same.** `cache_path` builds `<uuid>.<ext>` while `CatalogEntry::file_name` carries the
    /// camera's name for the grid to show. Writing `asset("one")` with `original_file_name: "one.jpg"`
    /// made the two coincide, so every test below passed while the real bug — the catalog keyed on the
    /// display name and looked up by the path — sat there untouched. **Reported in use: "pressing any
    /// of the buttons simply moves to the next picture, without leaving any marks".**
    fn asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            // ⚠ Deliberately nothing like `id`, the way a real asset's UUID is nothing like
            // `IMG-20231218-WA0011.jpg`.
            original_file_name: format!("IMG-2023-{id}-WA0011.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
            is_favorite: false,
            exif_info: None,
            people: None,
        }
    }

    fn marked(id: &str, favourite: bool, rating: Option<i32>) -> immich::Asset {
        immich::Asset {
            is_favorite: favourite,
            exif_info: Some(immich::ExifInfo {
                rating,
                ..Default::default()
            }),
            ..asset(id)
        }
    }

    /// M10.15c: Immich's own marks as Rustroom's — the author's mapping.
    #[test]
    fn the_server_marks_map_to_stars_pick_and_reject() {
        use catalog::db::Flag;
        for (fav, rating, want) in [
            (false, Some(3), (Some(3), None)),
            (false, Some(5), (Some(5), None)),
            (true, None, (None, Some(Flag::Pick))),
            (false, Some(-1), (None, Some(Flag::Reject))),
            (true, Some(-1), (None, Some(Flag::Pick))),
            (true, Some(4), (Some(4), Some(Flag::Pick))),
            (false, Some(0), (None, None)),
            (false, Some(6), (None, None)),
            (false, Some(-2), (None, None)),
            (false, None, (None, None)),
        ] {
            assert_eq!(
                server_marks(&marked("a", fav, rating)),
                want,
                "{fav} {rating:?}"
            );
        }
        assert_eq!(
            server_marks(&asset("plain")),
            (None, None),
            "no exifInfo at all"
        );
    }

    #[test]
    fn fill_marks_fills_the_listed_photos_once() {
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        let root = std::path::Path::new("/cache/server");
        let assets = [marked("a", true, Some(3)), marked("b", false, None)];
        let scan = crate::immich_source::entries_from_assets(&assets, root);
        record_assets(&cat, root, &scan).unwrap();
        assert_eq!(fill_marks(&cat, root, &assets).unwrap(), 2);
        let a = cat
            .marks(&crate::immich_source::cache_path(root, &assets[0]), 0)
            .unwrap();
        assert_eq!((a.rating, a.flag), (3, catalog::db::Flag::Pick));
        assert_eq!(
            fill_marks(&cat, root, &assets).unwrap(),
            0,
            "a second open fills nothing"
        );
    }

    /// M10.18b, constructed: an asset's `exifInfo` position becomes its server row; a `null` one fills
    /// nothing. ⚠ The JSON shape is the schema's — the GUI check confirms it on the author's server.
    #[test]
    fn an_immich_asset_with_a_position_fills_a_server_row() {
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        let root = std::path::Path::new("/cache/server");
        let ex: immich::ExifInfo =
            serde_json::from_str(r#"{"rating":null,"latitude":38.7,"longitude":-9.1}"#).unwrap();
        let none: immich::ExifInfo =
            serde_json::from_str(r#"{"rating":null,"latitude":null,"longitude":null}"#).unwrap();
        let assets = [
            immich::Asset {
                exif_info: Some(ex),
                ..asset("a")
            },
            immich::Asset {
                exif_info: Some(none),
                ..asset("b")
            },
        ];
        let scan = crate::immich_source::entries_from_assets(&assets, root);
        record_assets(&cat, root, &scan).unwrap();
        assert_eq!(fill_locations(&cat, root, &assets).unwrap(), 1);
        let at = |a| {
            cat.location_of(&crate::immich_source::cache_path(root, a))
                .unwrap()
        };
        assert_eq!(at(&assets[0]), Some((38.7, -9.1)));
        assert_eq!(at(&assets[1]), None, "a null position fills nothing");
    }

    /// M10.18c, constructed: an asset's named people become Immich faces; a hidden or unnamed one does not.
    #[test]
    fn an_assets_people_fill_immich_faces() {
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        let root = std::path::Path::new("/cache/server");
        let face = immich::FaceBox {
            image_width: 100.0,
            image_height: 100.0,
            bounding_box_x1: 10.0,
            bounding_box_y1: 10.0,
            bounding_box_x2: 30.0,
            bounding_box_y2: 30.0,
        };
        let person = |id: &str, name: &str, hidden: bool| immich::AssetPerson {
            id: id.into(),
            name: name.into(),
            is_hidden: hidden,
            faces: vec![face],
        };
        let assets = [immich::Asset {
            people: Some(vec![
                person("p1", "Anna", false),
                person("p2", "", false),
                person("p3", "Ben", true),
            ]),
            ..asset("a")
        }];
        let scan = crate::immich_source::entries_from_assets(&assets, root);
        record_assets(&cat, root, &scan).unwrap();
        assert_eq!(fill_faces(&cat, root, &assets).unwrap(), 1);
        let f = cat
            .faces_of(&crate::immich_source::cache_path(root, &assets[0]))
            .unwrap();
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].immich);
        assert_eq!(cat.people().unwrap()[0].name, "Anna");
        // The server now says nobody is in it: its Immich faces go. A list without the field changes nothing.
        let none = [immich::Asset {
            people: None,
            ..assets[0].clone()
        }];
        fill_faces(&cat, root, &none).unwrap();
        assert_eq!(
            cat.faces_of(&crate::immich_source::cache_path(root, &assets[0]))
                .unwrap()
                .len(),
            1
        );
        let nobody = [immich::Asset {
            people: Some(vec![]),
            ..assets[0].clone()
        }];
        fill_faces(&cat, root, &nobody).unwrap();
        assert!(cat
            .faces_of(&crate::immich_source::cache_path(root, &assets[0]))
            .unwrap()
            .is_empty());
    }

    fn marks(rating: u8) -> catalog::db::Marks {
        catalog::db::Marks {
            rating,
            ..Default::default()
        }
    }

    /// ⚠⚠ **A guard on the fixture, not on the code.** If these ever coincide again, every test below
    /// starts passing for the wrong reason — which is exactly what happened on 2026-09-23.
    #[test]
    fn the_fixture_keeps_the_cache_name_and_the_display_name_apart() {
        let a = asset("abc");
        let root = std::path::Path::new("/tmp/immich");
        let path = crate::immich_source::cache_path(root, &a);
        assert_ne!(
            path.file_name().unwrap().to_string_lossy(),
            a.original_file_name,
            "the cache path's name and the display name must differ, as they do on a real server"
        );
    }

    /// ⚠⚠ **The report: a rating on an Immich photo vanished on restart.** `set_marks` returns
    /// `Ok(())` and does nothing when no `photos` row exists, so without `record_assets` the mark was
    /// never stored — and nothing failed at the time.
    #[test]
    fn a_mark_on_an_immich_photo_survives_a_reopened_catalog() {
        let dir = temp_dir("persist");
        let db = dir.join("catalog.sqlite");
        let root = dir.join("immich");
        let scan = crate::immich_source::entries_from_assets(&[asset("one")], &root);
        let path = scan[0].path.clone();

        {
            let cat = catalog::db::Catalog::open(&db).unwrap();
            record_assets(&cat, &root, &scan).unwrap();
            cat.set_marks(&path, 0, marks(4)).unwrap();
        }
        let cat = catalog::db::Catalog::open(&db).unwrap();
        let found = cat.marks_in(&root).unwrap();
        assert_eq!(
            found
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, _, m)| m.rating),
            Some(4),
            "the rating must still be there after reopening the catalog"
        );
    }

    /// ⚠⚠ **One asset is one photo.** Reached through an album and through the timeline it must carry
    /// the *same* marks — which is why the folder is the server's cache root and not the album.
    #[test]
    fn an_asset_reached_two_ways_has_one_identity() {
        let dir = temp_dir("identity");
        let root = dir.join("immich");
        let cat = catalog::db::Catalog::open(&dir.join("c.sqlite")).unwrap();

        let from_album = crate::immich_source::entries_from_assets(&[asset("shared")], &root);
        let from_month =
            crate::immich_source::entries_from_assets(&[asset("shared"), asset("other")], &root);
        record_assets(&cat, &root, &from_album).unwrap();
        cat.set_marks(&from_album[0].path, 0, marks(5)).unwrap();
        record_assets(&cat, &root, &from_month).unwrap();

        let found = cat.marks_in(&root).unwrap();
        assert_eq!(
            found
                .iter()
                .find(|(p, _, _)| *p == from_album[0].path)
                .map(|(_, _, m)| m.rating),
            Some(5),
            "recording the month's list must not have given the asset a second, unmarked row"
        );
    }

    /// ⚠⚠ **A photo can be marked before it is downloaded.** The grid fills from server thumbnails, so
    /// rating straight from it — with no file on disk — has to work. `file_facts` records a `None` size
    /// for a missing file and the row is complete enough regardless.
    #[test]
    fn an_asset_with_no_file_on_disk_can_still_be_marked() {
        let dir = temp_dir("nofile");
        let root = dir.join("immich");
        let scan = crate::immich_source::entries_from_assets(&[asset("absent")], &root);
        assert!(
            !scan[0].path.exists(),
            "the fixture must not create the file"
        );

        let cat = catalog::db::Catalog::open(&dir.join("c.sqlite")).unwrap();
        record_assets(&cat, &root, &scan).unwrap();
        cat.set_marks(&scan[0].path, 0, marks(3)).unwrap();
        assert_eq!(cat.marks_in(&root).unwrap().len(), 1);
    }

    /// ⚠ Browsing a second album must not forget the first. `sync_folder` never deletes a row; this
    /// pins it at the level the feature depends on.
    #[test]
    fn recording_a_second_album_keeps_the_first_albums_rows() {
        let dir = temp_dir("accumulate");
        let root = dir.join("immich");
        let cat = catalog::db::Catalog::open(&dir.join("c.sqlite")).unwrap();
        let a = crate::immich_source::entries_from_assets(&[asset("a1"), asset("a2")], &root);
        let b = crate::immich_source::entries_from_assets(&[asset("b1")], &root);

        record_assets(&cat, &root, &a).unwrap();
        cat.set_marks(&a[0].path, 0, marks(2)).unwrap();
        record_assets(&cat, &root, &b).unwrap();

        assert_eq!(cat.photos_in(&root).unwrap().len(), 3, "all three rows");
        assert_eq!(
            cat.marks_in(&root).unwrap().len(),
            1,
            "and the mark survived"
        );
    }

    // ── shows_download_badge ──

    fn present_of(names: &[&str]) -> std::collections::HashSet<std::ffi::OsString> {
        names.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn a_downloaded_asset_under_an_immich_source_shows_the_badge() {
        let present = present_of(&["abc.jpg"]);
        assert!(shows_download_badge(
            true,
            Path::new("/cache/immich/host/abc.jpg"),
            &present
        ));
    }

    #[test]
    fn an_asset_not_yet_downloaded_shows_nothing() {
        let present = present_of(&["abc.jpg"]);
        assert!(!shows_download_badge(
            true,
            Path::new("/cache/immich/host/def.jpg"),
            &present
        ));
    }

    /// ⚠⚠ **Not under a folder source, even though the file is plainly there.** A local photo is
    /// always downloaded, so the mark would appear on every cell of every folder — which is the
    /// opposite of information.
    #[test]
    fn a_folder_source_shows_nothing_even_for_a_present_file() {
        let present = present_of(&["abc.jpg"]);
        assert!(!shows_download_badge(
            false,
            Path::new("/Users/me/Pictures/abc.jpg"),
            &present
        ));
    }

    // ── folder_display_name ──

    #[test]
    fn the_immich_cache_root_is_shown_as_immich_downloaded() {
        let root = Path::new("/c/rustroom/immich/https___host_example");
        assert_eq!(folder_display_name(root, Some(root)), "immich (downloaded)");
    }

    #[test]
    fn any_other_folder_keeps_its_basename() {
        let root = Path::new("/c/rustroom/immich/https___host_example");
        assert_eq!(
            folder_display_name(Path::new("/Users/me/Pictures/Iceland"), Some(root)),
            "Iceland"
        );
    }

    /// ⚠ No server renames nothing, and a folder that merely *looks* like a host slug is not the
    /// configured root and keeps its own name.
    #[test]
    fn nothing_is_renamed_without_the_configured_root() {
        let lookalike = Path::new("/somewhere/https___host_example");
        assert_eq!(folder_display_name(lookalike, None), "https___host_example");
        assert_eq!(
            folder_display_name(
                lookalike,
                Some(Path::new("/c/rustroom/immich/https___other"))
            ),
            "https___host_example"
        );
    }

    // ── row_is_browsable ──

    #[test]
    fn an_immich_row_with_no_file_is_not_browsable() {
        let dir = temp_dir("browsable");
        let root = dir.join("immich");
        assert!(!row_is_browsable(&root.join("nope.jpg"), Some(&root)));
    }

    #[test]
    fn an_immich_row_whose_file_exists_is_browsable() {
        let dir = temp_dir("present");
        let root = dir.join("immich");
        std::fs::create_dir_all(&root).unwrap();
        let there = root.join("yes.jpg");
        std::fs::write(&there, b"x").unwrap();
        assert!(row_is_browsable(&there, Some(&root)));
    }

    /// ⚠⚠ **The unplugged-drive rule is NOT being changed.** A local folder also keeps rows whose files
    /// are absent — deliberately, per `sync_folder`'s doc — and those must stay browsable. Only Immich
    /// rows are checked.
    #[test]
    fn a_local_row_with_no_file_is_still_browsable() {
        let dir = temp_dir("unplugged");
        let root = dir.join("immich");
        let elsewhere = dir.join("some_folder").join("gone.RAF");
        assert!(!elsewhere.exists());
        assert!(
            row_is_browsable(&elsewhere, Some(&root)),
            "an absent local file must not be hidden — that is a different question"
        );
    }

    /// ⚠ No server configured: nothing is an Immich path, so nothing is hidden.
    #[test]
    fn with_no_server_configured_every_row_is_browsable() {
        let dir = temp_dir("noserver");
        assert!(row_is_browsable(&dir.join("anything.jpg"), None));
    }

    /// Opening an album records each download's original name against its cache name — what Move to
    /// folder renames it to (2026-10-01).
    #[test]
    fn recording_assets_remembers_their_original_names() {
        let dir = temp_dir("names");
        let root = dir.join("immich");
        let cat = catalog::db::Catalog::open(&dir.join("c.sqlite")).unwrap();
        let scan = crate::immich_source::entries_from_assets(&[asset("one")], &root);
        let cached = scan[0].path.clone();
        assert_ne!(
            cached.file_name().unwrap().to_string_lossy(),
            "IMG-2023-one-WA0011.jpg",
            "the sample: the cache name is not the original"
        );
        record_assets(&cat, &root, &scan).unwrap();
        assert_eq!(
            cat.original_name(&cached).unwrap().as_deref(),
            Some("IMG-2023-one-WA0011.jpg")
        );
    }
}
