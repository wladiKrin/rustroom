use super::*;
use std::path::Path;

/// The settings file names each kind plainly (`"kind": "Folder"`), so it reads — and a renamed variant shows.
#[test]
fn every_source_kind_serialises_with_its_kind_tag() {
    let s = |n: &str| n.to_string();
    for (source, kind) in [
        (
            Source::Folder {
                path: PathBuf::from("/p"),
            },
            "Folder",
        ),
        (Source::AllPhotos, "AllPhotos"),
        (
            Source::Smart {
                id: 1,
                name: s("a"),
            },
            "Smart",
        ),
        (
            Source::Periods {
                periods: vec![(2026, None, None)],
                label: s("2026"),
            },
            "Periods",
        ),
        (
            Source::Collection {
                id: 1,
                name: s("a"),
            },
            "Collection",
        ),
        (
            Source::Publish {
                id: 1,
                name: s("a"),
            },
            "Publish",
        ),
        (
            Source::Keyword {
                id: 1,
                name: s("a"),
            },
            "Keyword",
        ),
        (
            Source::Person {
                id: 1,
                name: s("a"),
            },
            "Person",
        ),
        (
            Source::ImmichPerson {
                id: s("u"),
                person: 1,
                name: s("a"),
            },
            "ImmichPerson",
        ),
        (
            Source::ImmichAlbum {
                id: s("u"),
                name: s("a"),
            },
            "ImmichAlbum",
        ),
        (
            Source::ImmichBucket {
                bucket: s("b"),
                label: s("a"),
            },
            "ImmichBucket",
        ),
    ] {
        let v = serde_json::to_value(&source).unwrap();
        assert_eq!(v["kind"], kind, "{source:?}");
        let back: Source = serde_json::from_value(v).unwrap();
        assert_eq!(back, source);
    }
}

/// Constructed: a catalog of `files` in `dir`, read into a `RailData` as the rail would.
fn rail_over(dir: &Path, files: &[&str]) -> (catalog::db::Catalog, RailData) {
    for f in files {
        std::fs::write(dir.join(f), b"x").unwrap();
    }
    let cat = catalog::db::Catalog::open_in_memory().unwrap();
    let scan = catalog::scan_folder(dir).unwrap();
    cat.sync_folder(dir, &scan).unwrap();
    let mut rail = RailData::default();
    rail.refresh(Some(&cat), None);
    (cat, rail)
}

#[test]
fn a_folder_that_exists_or_is_catalogued_reopens() {
    let d = test_dir::TempDir::new("start-folder");
    let (_cat, rail) = rail_over(d.path(), &["a.jpg"]);
    assert!(
        !rail.folders.is_empty(),
        "fixture check: the catalog lists the folder"
    );
    let folder = |p: &Path| Source::Folder {
        path: p.to_path_buf(),
    };
    // On disk, and catalogued.
    assert!(reopenable(&folder(d.path()), &rail, false, |p| p.is_dir()));
    // Gone from the disk (an unplugged drive) but still catalogued: the rail would open it, so this does.
    assert!(reopenable(&folder(d.path()), &rail, false, |_| false));
    // Neither.
    let other = d.join("never-seen");
    assert!(!reopenable(&folder(&other), &rail, false, |p| p.is_dir()));
    // On disk only (opened through File → Open folder before the catalog knew it).
    std::fs::create_dir(&other).unwrap();
    assert!(reopenable(&folder(&other), &rail, false, |p| p.is_dir()));
}

#[test]
fn a_deleted_collection_does_not_reopen() {
    let d = test_dir::TempDir::new("start-collection");
    let (cat, _) = rail_over(d.path(), &["a.jpg"]);
    let id = cat.create_collection("Portfolio").unwrap();
    let mut rail = RailData::default();
    rail.refresh(Some(&cat), None);
    let c = Source::Collection {
        id,
        name: "Portfolio".to_string(),
    };
    assert!(reopenable(&c, &rail, false, |_| true));
    cat.delete_collection(id).unwrap();
    rail.refresh(Some(&cat), None);
    assert!(!reopenable(&c, &rail, false, |_| true));
}

#[test]
fn immich_sources_need_a_connection() {
    let rail = RailData::default();
    let album = Source::ImmichAlbum {
        id: "u".to_string(),
        name: "Trip".to_string(),
    };
    let bucket = Source::ImmichBucket {
        bucket: "2026-11-01".to_string(),
        label: "November 2026".to_string(),
    };
    for s in [&album, &bucket] {
        assert!(!reopenable(s, &rail, false, |_| true));
        assert!(reopenable(s, &rail, true, |_| true));
    }
    // All Photos and dates always reopen, though they may show nothing.
    assert!(reopenable(&Source::AllPhotos, &rail, false, |_| false));
}

#[test]
fn continue_lists_all_photos_then_five_existing_recents() {
    let d = test_dir::TempDir::new("start-continue");
    let (_cat, rail) = rail_over(d.path(), &["a.jpg", "b.jpg"]);
    let recent: Vec<PathBuf> = (0..7).map(|i| d.join(format!("r{i}"))).collect();
    let mut recent = recent;
    recent.insert(1, d.path().to_path_buf());
    let gone = recent[3].clone();
    let rows = continue_rows(&recent, &rail, 2, |p| p != gone && p.starts_with(d.path()));
    assert_eq!(rows[0].kind, ContinueKind::AllPhotos);
    assert_eq!(rows[0].count, Some(2));
    let folders: Vec<&PathBuf> = rows[1..]
        .iter()
        .map(|r| match &r.kind {
            ContinueKind::Folder(p) => p,
            ContinueKind::AllPhotos => panic!("All Photos twice"),
        })
        .collect();
    assert_eq!(folders.len(), CONTINUE_MAX);
    let want: Vec<&PathBuf> = recent.iter().filter(|p| **p != gone).take(5).collect();
    assert_eq!(folders, want);
    // The catalogued folder carries its count; one the catalog has never seen has none.
    assert_eq!(rows[2].count, Some(2));
    assert_eq!(rows[1].count, None);
    assert_eq!(
        rows[2].name,
        d.path().file_name().unwrap().to_string_lossy()
    );
    assert!(rows[2].parent.is_some());
}

#[test]
fn an_empty_catalog_and_no_recents_has_nothing_to_continue() {
    let rail = RailData::default();
    assert!(continue_rows(&[], &rail, 0, |_| true).is_empty());
}

/// ⚠⚠ The review's #3: the start page draws every frame, and `is_dir` on a stale network mount can block for
/// seconds — so the rows are derived once per input, not per frame.
#[test]
fn the_continue_rows_are_checked_once_per_change() {
    let rail = RailData::default();
    let calls = std::cell::Cell::new(0);
    let exists = |_: &Path| {
        calls.set(calls.get() + 1);
        true
    };
    let recent = vec![PathBuf::from("/a"), PathBuf::from("/b")];
    let mut memo = ContinueMemo::default();
    assert_eq!(memo.rows(&recent, &rail, 0, exists).len(), 2);
    assert_eq!(memo.rows(&recent, &rail, 0, exists).len(), 2);
    assert_eq!(calls.get(), 2, "two folders checked once, not once a frame");
    let more = vec![
        PathBuf::from("/c"),
        PathBuf::from("/a"),
        PathBuf::from("/b"),
    ];
    assert_eq!(memo.rows(&more, &rail, 0, exists).len(), 3);
    assert_eq!(calls.get(), 5, "a new recent list is checked again");
    memo.clear();
    let _ = memo.rows(&more, &rail, 0, exists);
    assert_eq!(calls.get(), 8, "and so is a page shown again");
}
