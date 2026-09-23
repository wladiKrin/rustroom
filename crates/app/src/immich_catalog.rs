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
/// ⚠ A one-line wrapper on purpose: this is where the reasoning above lives and what the tests
/// address. If it is still one line after another slice, fold it into the callers.
pub fn record_assets(
    cat: &catalog::db::Catalog,
    root: &Path,
    scan: &[catalog::CatalogEntry],
) -> Result<(), catalog::db::CatalogError> {
    cat.sync_folder(root, scan)
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

    /// ⚠ Its own directory per test, keyed by process id **and a counter** — the collision
    /// `thumb_store::TempDir`'s doc says has already bitten this repo twice.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_imcat_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("{id}.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
        }
    }

    fn marks(rating: u8) -> catalog::db::Marks {
        catalog::db::Marks {
            rating,
            ..Default::default()
        }
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
        assert!(!scan[0].path.exists(), "the fixture must not create the file");

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
        assert_eq!(cat.marks_in(&root).unwrap().len(), 1, "and the mark survived");
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
}
