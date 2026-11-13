use super::*;
use rustroom_app::start::{Place, Source};
use std::path::{Path, PathBuf};

/// Constructed: a folder of `n` photos, catalogued, and an app over that catalog.
fn app_with_folder(tag: &str, n: usize) -> (RustroomApp, test_dir::TempDir, Vec<PathBuf>) {
    let mut app = crate::test_app::for_test();
    let d = test_dir::TempDir::new(tag);
    let files: Vec<PathBuf> = (0..n).map(|i| d.join(format!("p{i}.jpg"))).collect();
    for f in &files {
        std::fs::write(f, b"x").unwrap();
    }
    let cat = catalog::db::Catalog::open_in_memory().unwrap();
    cat.sync_folder(d.path(), &catalog::scan_folder(d.path()).unwrap())
        .unwrap();
    app.catalog = Some(cat);
    (app, d, files)
}

fn place(source: Source, selected: Option<&Path>) -> Place {
    Place {
        source,
        selected: selected.map(|p| (p.to_path_buf(), 0)),
    }
}

fn folder(p: &Path) -> Source {
    Source::Folder {
        path: p.to_path_buf(),
    }
}

/// The photo the Library has selected, alone.
fn selected(app: &RustroomApp) -> Vec<PathBuf> {
    app.library
        .as_ref()
        .unwrap()
        .selected_ids()
        .into_iter()
        .map(|id| id.path)
        .collect()
}

fn source(app: &RustroomApp) -> Option<LibrarySource> {
    app.library.as_ref().map(|l| l.source.clone())
}

#[test]
fn a_folder_reopens_with_its_photo_selected() {
    let (mut app, d, files) = app_with_folder("place-folder", 4);
    app.settings.last_place = Some(place(folder(d.path()), Some(&files[2])));
    app.pump_place(&egui::Context::default());
    assert_eq!(
        source(&app),
        Some(LibrarySource::Folder(d.path().to_path_buf()))
    );
    assert_eq!(selected(&app), vec![files[2].clone()]);
    assert!(
        app.library.as_ref().unwrap().reveal,
        "the grid scrolls to it"
    );
    assert_eq!(app.place_state, PlaceState::Mirroring);
}

#[test]
fn all_photos_reopens() {
    let (mut app, _d, files) = app_with_folder("place-all", 3);
    app.settings.last_place = Some(place(Source::AllPhotos, Some(&files[1])));
    app.pump_place(&egui::Context::default());
    assert_eq!(source(&app), Some(LibrarySource::AllPhotos));
    assert_eq!(selected(&app), vec![files[1].clone()]);
}

#[test]
fn a_collection_reopens() {
    let (mut app, _d, files) = app_with_folder("place-collection", 3);
    let cat = app.catalog.as_ref().unwrap();
    let id = cat.create_collection("Portfolio").unwrap();
    assert!(cat.add_to_collection(id, &files[1], 0).unwrap());
    let name = "Portfolio".to_string();
    app.settings.last_place = Some(place(
        Source::Collection {
            id,
            name: name.clone(),
        },
        Some(&files[1]),
    ));
    app.pump_place(&egui::Context::default());
    assert_eq!(source(&app), Some(LibrarySource::Collection { id, name }));
    assert_eq!(selected(&app), vec![files[1].clone()]);
}

/// Constructed asset: one photo of an album (`immich_flow`'s fixture shape).
fn asset(id: &str) -> immich::Asset {
    immich::Asset {
        id: id.to_string(),
        asset_type: immich::AssetType::Image,
        original_file_name: format!("{id}.jpg"),
        original_mime_type: None,
        width: Some(100),
        height: Some(100),
        thumbhash: None,
        file_modified_at: "2025-08-14T09:21:33.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        is_favorite: false,
        exif_info: None,
        people: None,
    }
}

#[test]
fn an_immich_album_reopens_from_the_session_list() {
    let mut app = crate::test_app::for_test();
    app.settings.immich_url = "https://example.invalid".to_string();
    app.settings.immich_key = "key".to_string();
    assert!(
        rustroom_app::immich_source::connection(&app.settings).is_some(),
        "fixture check: Immich is connected"
    );
    app.immich_assets
        .insert("al".to_string(), vec![asset("a1"), asset("a2")]);
    let album = Source::ImmichAlbum {
        id: "al".to_string(),
        name: "Trip".to_string(),
    };
    app.settings.last_place = Some(place(album, None));
    app.pump_place(&egui::Context::default());
    assert_eq!(
        source(&app),
        Some(LibrarySource::ImmichAlbum {
            id: "al".to_string(),
            name: "Trip".to_string()
        })
    );
}

#[test]
fn a_deleted_collection_shows_the_start_page() {
    let (mut app, _d, _) = app_with_folder("place-deleted", 2);
    app.settings.last_place = Some(place(
        Source::Collection {
            id: 9999,
            name: "Gone".to_string(),
        },
        None,
    ));
    app.pump_place(&egui::Context::default());
    assert!(app.library.is_none());
    assert_eq!(app.place_state, PlaceState::Mirroring);
}

#[test]
fn a_gone_uncatalogued_folder_shows_the_start_page() {
    let (mut app, d, _) = app_with_folder("place-gone", 1);
    app.settings.last_place = Some(place(folder(&d.join("never")), None));
    app.pump_place(&egui::Context::default());
    assert!(app.library.is_none());
    assert_eq!(app.place_state, PlaceState::Mirroring);
}

#[test]
fn reopening_off_shows_the_start_page() {
    let (mut app, d, _) = app_with_folder("place-off", 2);
    app.settings.reopen_last = false;
    app.settings.last_place = Some(place(folder(d.path()), None));
    app.pump_place(&egui::Context::default());
    assert!(app.library.is_none());
}

/// ⚠⚠ The first frame's empty Library must never overwrite the place: with reopening off nothing opens, and the
/// stored place must survive the frames that follow until the Library actually changes.
#[test]
fn the_mirror_does_not_write_before_the_reopen() {
    let (mut app, d, _) = app_with_folder("place-first", 2);
    app.settings.reopen_last = false;
    let stored = place(folder(d.path()), None);
    app.settings.last_place = Some(stored.clone());
    app.pump_place(&egui::Context::default());
    assert_eq!(app.settings.last_place, Some(stored));
}

#[test]
fn closing_the_library_stores_nothing_open() {
    let (mut app, d, _) = app_with_folder("place-close", 2);
    let ctx = egui::Context::default();
    app.pump_place(&ctx);
    app.open_catalog_folder(&ctx, d.path());
    app.pump_place(&ctx);
    assert_eq!(
        app.settings.last_place.as_ref().map(|p| &p.source),
        Some(&folder(d.path())),
        "the opened folder is mirrored"
    );
    app.library = None;
    app.pump_place(&ctx);
    assert!(app.settings.last_place.is_none());
}

#[test]
fn a_failed_immich_reopen_leaves_the_start_page_and_resumes_the_mirror() {
    let mut app = crate::test_app::for_test();
    app.settings.immich_url = "https://example.invalid".to_string();
    app.settings.immich_key = "key".to_string();
    let album = place(
        Source::ImmichAlbum {
            id: "not-in-hand".to_string(),
            name: "Trip".to_string(),
        },
        None,
    );
    app.settings.last_place = Some(album.clone());
    let ctx = egui::Context::default();
    app.pump_place(&ctx);
    assert_eq!(app.place_state, PlaceState::Pending(album.clone()));
    app.pump_place(&ctx);
    assert!(
        matches!(app.place_state, PlaceState::Pending(_)),
        "still loading"
    );
    assert_eq!(
        app.settings.last_place,
        Some(album),
        "not stored as nothing open"
    );
    // The reply failed: `pump_immich` clears `immich_opening` and opens nothing.
    app.immich_opening = None;
    app.pump_place(&ctx);
    assert_eq!(app.place_state, PlaceState::Mirroring);
    assert!(app.library.is_none());
}

#[test]
fn a_new_source_before_the_reopen_lands_drops_the_selection() {
    let (mut app, d, files) = app_with_folder("place-switch", 3);
    app.settings.immich_url = "https://example.invalid".to_string();
    app.settings.immich_key = "key".to_string();
    let ctx = egui::Context::default();
    app.settings.last_place = Some(Place {
        source: Source::ImmichAlbum {
            id: "not-in-hand".to_string(),
            name: "Trip".to_string(),
        },
        selected: Some((files[2].clone(), 0)),
    });
    app.pump_place(&ctx);
    assert!(matches!(app.place_state, PlaceState::Pending(_)));
    app.open_catalog_folder(&ctx, d.path());
    let default = selected(&app);
    app.pump_place(&ctx);
    assert_eq!(app.place_state, PlaceState::Mirroring);
    assert_eq!(selected(&app), default);
}

#[test]
fn a_missing_photo_keeps_the_default_selection() {
    let (mut app, d, _) = app_with_folder("place-missing", 3);
    let ctx = egui::Context::default();
    let mut plain = crate::test_app::for_test();
    plain.catalog = app.catalog.take();
    plain.open_catalog_folder(&ctx, d.path());
    let default = selected(&plain);
    app.catalog = plain.catalog.take();
    app.settings.last_place = Some(place(folder(d.path()), Some(&d.join("not-here.jpg"))));
    app.pump_place(&ctx);
    assert_eq!(selected(&app), default);
    assert!(!app.library.as_ref().unwrap().reveal);
}

#[test]
fn close_remembers_the_develop_photo() {
    let (mut app, d, files) = app_with_folder("place-develop", 3);
    let ctx = egui::Context::default();
    app.pump_place(&ctx);
    app.open_catalog_folder(&ctx, d.path());
    app.library.as_mut().unwrap().selection.click(0);
    crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
    app.session.as_mut().unwrap().path = files[1].clone();
    app.remember_place_on_close();
    assert_eq!(
        app.settings.last_place.unwrap().selected,
        Some((files[1].clone(), 0))
    );
}

#[test]
fn every_source_converts_both_ways() {
    let s = |n: &str| n.to_string();
    for source in [
        LibrarySource::Folder(PathBuf::from("/p")),
        LibrarySource::AllPhotos,
        LibrarySource::Smart {
            id: 1,
            name: s("a"),
        },
        LibrarySource::Periods {
            periods: vec![(2026, Some(11), None)],
            label: s("November 2026"),
        },
        LibrarySource::Collection {
            id: 2,
            name: s("a"),
        },
        LibrarySource::Publish {
            id: 3,
            name: s("a"),
        },
        LibrarySource::Keyword {
            id: 4,
            name: s("a"),
        },
        LibrarySource::Person {
            id: 5,
            name: s("a"),
        },
        LibrarySource::ImmichPerson {
            id: s("u"),
            person: 6,
            name: s("a"),
        },
        LibrarySource::ImmichAlbum {
            id: s("u"),
            name: s("a"),
        },
        LibrarySource::ImmichBucket {
            bucket: s("b"),
            label: s("a"),
        },
    ] {
        assert_eq!(from_place(&to_place(&source)), source);
    }
}

/// The window's close, on every platform: the place is stored with the photo that was selected.
#[test]
fn a_close_request_remembers_the_place() {
    let (mut app, d, files) = app_with_folder("place-close-request", 3);
    let ctx = egui::Context::default();
    app.pump_place(&ctx);
    app.open_catalog_folder(&ctx, d.path());
    app.pump_place(&ctx);
    let i = app
        .library
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .position(|e| e.id.path == files[2])
        .unwrap();
    app.library.as_mut().unwrap().selection.click(i);
    app.on_close_requested();
    assert_eq!(
        app.settings.last_place,
        Some(place(folder(d.path()), Some(&files[2])))
    );
}
