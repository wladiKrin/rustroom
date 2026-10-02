//! Folders and the photos in them: syncing a scan, renaming, copies, and the listings.

use super::{file_facts, split, unix_now, Catalog, CatalogError};
use crate::CatalogEntry;
use std::path::{Path, PathBuf};

/// One photo row, as [`Catalog::all_photos`] returns it.
///
/// ⚠ Carries `mtime` **from the catalog**, recorded at scan time. That column has existed since
/// M4.1 and nothing has ever read it; a whole-library view is what it was captured for, because
/// walking the filesystem for 10,000 photos is exactly what this view must not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoRow {
    pub path: PathBuf,
    pub copy: u32,
    /// Unix seconds, or `None` if the metadata could not be read when the folder was scanned.
    pub mtime: Option<i64>,
}

/// One catalogued folder, with how many photo rows it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: i64,
    pub path: PathBuf,
    pub count: i64,
}

impl Catalog {
    /// Record `folder` and every photo in `files`, adding what is new and refreshing what is not.
    ///
    /// ⚠⚠ **This never deletes a photo row.** A folder on an unplugged drive scans as empty, and
    /// removing rows then would silently empty every collection referencing them. `last_seen`
    /// records what was present; absence is not deletion.
    ///
    /// ⚠ One transaction. Half a folder recorded is worse than none, and it is also much faster:
    /// SQLite commits per statement otherwise, which is a disk sync per photo.
    pub fn sync_folder(&self, folder: &Path, files: &[CatalogEntry]) -> Result<(), CatalogError> {
        let now = unix_now();
        let folder_key = folder.to_string_lossy().to_string();
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| -> Result<(), CatalogError> {
            self.conn.execute(
                "INSERT INTO folders (path, last_seen) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET last_seen = ?2",
                rusqlite::params![folder_key, now],
            )?;
            let folder_id: i64 = self.conn.query_row(
                "SELECT id FROM folders WHERE path = ?1",
                [&folder_key],
                |r| r.get(0),
            )?;
            for f in files {
                let (size, mtime) = file_facts(f);
                // ⚠⚠ **Keyed on the PATH's own name, never on `CatalogEntry::file_name`.** Every read
                // side — `photo_id`, `marks_in`, `all_photos` — derives the key from a `Path` through
                // `split`, so a row written under a different name can never be found again.
                //
                // For a local file the two are identical, which is why this went unnoticed. An Immich
                // asset's cache path is `<uuid>.<ext>` while its `file_name` is the camera's name for
                // the grid to show, so the row was written as `IMG-20231218-WA0011.jpg` and looked up
                // as `<uuid>.jpg`. `set_marks` found nothing and returned `Ok(())`, so **every rating
                // on an Immich photo was silently dropped** — reported in use 2026-09-23 as "pressing
                // any of the buttons simply moves to the next picture, without leaving any marks".
                let Some((_, key)) = split(&f.path) else {
                    continue;
                };
                self.conn.execute(
                    "INSERT INTO photos (folder_id, file_name, copy, size, mtime, last_seen)
                     VALUES (?1, ?2, 0, ?3, ?4, ?5)
                     ON CONFLICT(folder_id, file_name, copy)
                     DO UPDATE SET size = ?3, mtime = ?4, last_seen = ?5",
                    rusqlite::params![folder_id, key, size, mtime, now],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// The file names recorded for `folder`.
    pub fn photos_in(&self, folder: &Path) -> Result<Vec<String>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name FROM photos p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map([&key], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn folder_count(&self) -> Result<i64, CatalogError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM folders", [], |r| r.get(0))?)
    }

    pub fn photo_count(&self) -> Result<i64, CatalogError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM photos", [], |r| r.get(0))?)
    }

    /// The row id for one version of one file, if the catalog knows it.
    pub fn photo_id(&self, photo: &Path, copy: u32) -> Result<Option<i64>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT p.id FROM photos p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND p.file_name = ?2 AND p.copy = ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![folder, name, copy])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// Every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠ Ordered by folder then name so the result is stable; the Library re-sorts it by the
    /// user's chosen order anyway, but a stable base makes that sort's tie-break meaningful.
    pub fn all_photos(&self) -> Result<Vec<PhotoRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy, p.mtime
             FROM photos p JOIN folders f ON f.id = p.folder_id
             ORDER BY f.path, p.file_name, p.copy",
        )?;
        let rows = stmt.query_map([], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            Ok(PhotoRow {
                path: PathBuf::from(folder).join(name),
                copy: r.get(2)?,
                mtime: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every catalogued folder, with its photo count.
    pub fn folders(&self) -> Result<Vec<FolderRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.path, (SELECT count(*) FROM photos p WHERE p.folder_id = f.id)
             FROM folders f
             ORDER BY lower(f.path), f.path",
        )?;
        let rows = stmt.query_map([], |r| {
            let path: String = r.get(1)?;
            Ok(FolderRow {
                id: r.get(0)?,
                path: PathBuf::from(path),
                count: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Record a virtual copy of an already-known file.
    ///
    /// ⚠ Copies never come from a folder scan — they live in the sidecar — so they are added here
    /// explicitly when one is created.
    pub fn add_copy(&self, photo: &Path, copy: u32) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO photos (folder_id, file_name, copy, last_seen)
             SELECT f.id, ?2, ?3, ?4 FROM folders f WHERE f.path = ?1
             ON CONFLICT(folder_id, file_name, copy) DO UPDATE SET last_seen = ?4",
            rusqlite::params![folder, name, copy, now],
        )?;
        Ok(())
    }

    /// Follow a `Reindex` renumbering of one copy.
    pub fn renumber_copy(&self, photo: &Path, from: u32, to: u32) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE photos SET copy = ?4
             WHERE file_name = ?2 AND copy = ?3
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![folder, name, from, to],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{entries_in, entry};

    #[test]
    fn syncing_a_folder_records_its_photos() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let mut got = cat.photos_in(Path::new("/x")).unwrap();
        got.sort();
        assert_eq!(got, vec!["a.RAF".to_string(), "b.RAF".to_string()]);
    }

    #[test]
    fn syncing_the_same_folder_twice_does_not_duplicate_it() {
        // ⚠ Opening a folder is the *normal* case, not an exceptional one — someone browsing
        // their library re-opens the same folder daily. Duplicated rows would multiply forever
        // and every collection query would return the same photo many times.
        let cat = Catalog::open_in_memory().unwrap();
        let files = [entry("a.RAF")];
        cat.sync_folder(Path::new("/x"), &files).unwrap();
        cat.sync_folder(Path::new("/x"), &files).unwrap();
        assert_eq!(cat.photos_in(Path::new("/x")).unwrap().len(), 1);
        assert_eq!(cat.folder_count().unwrap(), 1);
    }

    #[test]
    fn two_folders_with_a_same_named_file_are_two_photos() {
        // ⚠ `DSCF0001.RAF` exists in every folder a Fuji has ever filled. Keying on file name
        // alone would merge them; the UNIQUE constraint is on (folder, name, copy).
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("DSCF0001.RAF")])
            .unwrap();
        cat.sync_folder(Path::new("/y"), &[entry("DSCF0001.RAF")])
            .unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2);
    }

    #[test]
    fn a_folder_whose_files_have_vanished_keeps_its_rows() {
        // ⚠⚠ **The most important test in this plan.** An unplugged drive or an unmounted share
        // scans as empty. Deleting rows then would silently empty every collection referencing
        // them — and collections have no UI yet, so nothing would catch it. A scan adds and
        // updates; it never removes.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.sync_folder(Path::new("/x"), &[]).unwrap();
        assert_eq!(
            cat.photos_in(Path::new("/x")).unwrap().len(),
            2,
            "an empty scan must not delete anything"
        );
    }

    #[test]
    fn renaming_a_file_moves_its_row_rather_than_adding_one() {
        // ⚠⚠ Rustroom renames files itself. If the catalog is not told, the row keeps the old
        // name, the next scan adds a *second* row under the new one, and any collection
        // membership stays attached to a photo that no longer exists.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let id = cat.photo_id(Path::new("/x/a.RAF"), 0).unwrap().unwrap();

        cat.move_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(cat.photo_count().unwrap(), 1, "no second row");
        assert_eq!(
            cat.photo_id(Path::new("/x/b.RAF"), 0).unwrap(),
            Some(id),
            "the same row, under the new name"
        );
        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 0).unwrap(), None);
    }

    #[test]
    fn renaming_moves_every_copy_of_the_file() {
        // ⚠ Copies share the file, so a rename moves all of them. Moving only copy 0 would
        // strand the rest under a name that no longer exists.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 2).unwrap();

        cat.move_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        for copy in [0, 1, 2] {
            assert!(
                cat.photo_id(Path::new("/x/b.RAF"), copy).unwrap().is_some(),
                "copy {copy} did not follow the rename"
            );
        }
        assert_eq!(cat.photo_count().unwrap(), 3);
    }

    #[test]
    fn renumbering_a_copy_moves_its_row() {
        // ⚠ `Reindex` closes gaps in copy numbering. The catalog must follow, or the row for the
        // old number becomes unreachable while a new one appears empty.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 3).unwrap();
        let id = cat.photo_id(Path::new("/x/a.RAF"), 3).unwrap().unwrap();

        cat.renumber_copy(Path::new("/x/a.RAF"), 3, 1).unwrap();

        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 1).unwrap(), Some(id));
        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 3).unwrap(), None);
        assert_eq!(cat.photo_count().unwrap(), 2);
    }

    #[test]
    fn renaming_a_photo_the_catalog_has_never_seen_is_harmless() {
        // ⚠ The catalog is not authoritative and must never be a reason a rename fails. A photo
        // in a folder that was never opened simply has no row to move.
        let cat = Catalog::open_in_memory().unwrap();
        cat.move_photo(Path::new("/x/ghost.RAF"), Path::new("/x/other.RAF"))
            .expect("must not error");
        assert_eq!(cat.photo_count().unwrap(), 0);
    }

    #[test]
    fn all_photos_spans_every_folder() {
        // ⚠ The point of the whole slice: `open_folder`, `open_collection` and `open_keyword`
        // were the only loaders, so the catalog held everything and nothing could see it whole.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        let rows = cat.all_photos().unwrap();
        assert_eq!(rows.len(), 3);
        let mut paths: Vec<String> = rows
            .iter()
            .map(|r| r.path.to_string_lossy().to_string())
            .collect();
        paths.sort();
        assert_eq!(paths, ["/a/one.RAF", "/a/two.RAF", "/b/three.RAF"]);
    }

    #[test]
    fn all_photos_includes_every_copy() {
        // A virtual copy is its own row and its own identity — it must appear here as one.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        let rows = cat.all_photos().unwrap();
        assert_eq!(rows.len(), 2);
        let mut copies: Vec<u32> = rows.iter().map(|r| r.copy).collect();
        copies.sort();
        assert_eq!(copies, [0, 1]);
    }

    #[test]
    fn folders_are_listed_with_their_photo_counts() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        let rows = cat.folders().unwrap();
        assert_eq!(rows.len(), 2);
        let by_path: Vec<(String, i64)> = rows
            .iter()
            .map(|f| (f.path.to_string_lossy().to_string(), f.count))
            .collect();
        assert!(by_path.contains(&("/a".to_string(), 2)), "{by_path:?}");
        assert!(by_path.contains(&("/b".to_string(), 1)), "{by_path:?}");
    }
}
