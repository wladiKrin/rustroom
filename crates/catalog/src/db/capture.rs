//! What the camera recorded (capture time, camera, lens) and the photo's description, per file.

use super::{split, Catalog, CatalogError};
use core_image::Capture;
use std::path::{Path, PathBuf};

/// What a photograph says about itself, in words: the four IPTC-shaped fields Rustroom edits.
///
/// ⚠⚠ **Per FILE, not per copy** — keyed like `photo_capture` and `photo_keywords`. A title says
/// what is *in* the picture, which every copy shares; only marks describe the image you made.
/// Lightroom gives virtual copies independent metadata; this is a recorded divergence (spec §1).
///
/// ⚠ Stored in the catalog only, and written into **exports** and the **XMP sidecar** — never
/// into an original. That is what dissolved the design conflict `docs/BACKLOG.md` held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Description {
    pub title: String,
    pub caption: String,
    pub copyright: String,
    pub creator: String,
}

impl Description {
    /// Whether every field is blank.
    ///
    /// ⚠ Trimmed, so a field holding only spaces counts as absent — the rule
    /// `ExportMetadata::is_empty` follows, and for the same reason: a tag whose whole content is
    /// whitespace is the blank-copyright problem in a costume.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.caption.trim().is_empty()
            && self.copyright.trim().is_empty()
            && self.creator.trim().is_empty()
    }
}

impl Catalog {
    /// Record what a file's EXIF said.
    ///
    /// ⚠⚠ **An all-`None` `Capture` is still stored.** The row's presence is what marks the file
    /// as read; skipping it would make the background pass re-read that file on every folder open
    /// forever.
    pub fn set_capture(&self, photo: &Path, c: &Capture) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO photo_capture
                 (folder_id, file_name, taken, camera, lens, iso, shutter, aperture, focal_len)
             SELECT id, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9 FROM folders WHERE path = ?1
             ON CONFLICT(folder_id, file_name) DO UPDATE SET
                 taken = ?3, camera = ?4, lens = ?5, iso = ?6,
                 shutter = ?7, aperture = ?8, focal_len = ?9",
            rusqlite::params![
                folder,
                name,
                c.taken,
                c.camera,
                c.lens,
                c.iso,
                c.shutter,
                c.aperture,
                c.focal_len,
            ],
        )?;
        Ok(())
    }

    /// What was recorded for a file, or `None` if it has never been read.
    ///
    /// ⚠ `Some(Capture::default())` and `None` mean different things: read-and-had-nothing versus
    /// never-read.
    pub fn capture_of(&self, photo: &Path) -> Result<Option<Capture>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let row = self.conn.query_row(
            "SELECT c.taken, c.camera, c.lens, c.iso, c.shutter, c.aperture, c.focal_len
             FROM photo_capture c JOIN folders f ON f.id = c.folder_id
             WHERE f.path = ?1 AND c.file_name = ?2",
            rusqlite::params![folder, name],
            |r| {
                Ok(Capture {
                    taken: r.get(0)?,
                    camera: r.get(1)?,
                    lens: r.get(2)?,
                    iso: r.get::<_, Option<i64>>(3)?.map(|v| v as u32),
                    shutter: r.get::<_, Option<f64>>(4)?.map(|v| v as f32),
                    aperture: r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    focal_len: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                })
            },
        );
        match row {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// What a photo says about itself, or `None` when it says nothing.
    ///
    /// ⚠ `None` is the ordinary state, not an error: only photos with a description have a row.
    pub fn description_of(&self, photo: &Path) -> Result<Option<Description>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let row = self.conn.query_row(
            "SELECT d.title, d.caption, d.copyright, d.creator
             FROM photo_description d JOIN folders f ON f.id = d.folder_id
             WHERE f.path = ?1 AND d.file_name = ?2",
            rusqlite::params![folder, name],
            |r| {
                Ok(Description {
                    title: r.get(0)?,
                    caption: r.get(1)?,
                    copyright: r.get(2)?,
                    creator: r.get(3)?,
                })
            },
        );
        match row {
            Ok(d) => Ok(Some(d)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record what a photo says about itself.
    ///
    /// ⚠ An all-blank description **deletes** the row rather than storing four empty strings,
    /// so "absent" and "blank" are one state — see `Description::is_empty`.
    ///
    /// ⚠ A photo whose folder the catalog does not know is silently skipped: the catalog is not
    /// authoritative and must never be a reason an action fails — the `add_to_collection` rule.
    pub fn set_description(&self, photo: &Path, d: &Description) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        if d.is_empty() {
            self.conn.execute(
                "DELETE FROM photo_description
                 WHERE file_name = ?2
                   AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
                rusqlite::params![folder, name],
            )?;
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO photo_description
                 (folder_id, file_name, title, caption, copyright, creator)
             SELECT id, ?2, ?3, ?4, ?5, ?6 FROM folders WHERE path = ?1
             ON CONFLICT(folder_id, file_name) DO UPDATE SET
                 title = ?3, caption = ?4, copyright = ?5, creator = ?6",
            rusqlite::params![folder, name, d.title, d.caption, d.copyright, d.creator],
        )?;
        Ok(())
    }

    /// The file names in `folder` with no capture row yet — the background pass's queue.
    ///
    /// ⚠ **DISTINCT on `file_name`**: `photos` has one row per (file, copy) while capture is per
    /// file, so without it a photo with three copies would be queued and read three times.
    pub fn files_without_capture(&self, folder: &Path) -> Result<Vec<String>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.file_name
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1
               AND NOT EXISTS (SELECT 1 FROM photo_capture c
                               WHERE c.folder_id = p.folder_id AND c.file_name = p.file_name)
             ORDER BY p.file_name",
        )?;
        let rows = stmt.query_map([&key], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every capture row in a folder, as `(path, capture)`.
    ///
    /// ⚠⚠ One query per folder per rebuild, never one per photo — the rule `marks_in` follows,
    /// because the grid draws sixty times a second over hundreds of thumbnails.
    ///
    /// ⚠ Keyed by path, so a file with several copies appears once: capture is per file.
    /// ⚠ Only files that have been *read* come back; the rest are simply absent.
    pub fn captures_in(&self, folder: &Path) -> Result<Vec<(PathBuf, Capture)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT c.file_name, c.taken, c.camera, c.lens, c.iso, c.shutter, c.aperture,
                    c.focal_len
             FROM photo_capture c JOIN folders f ON f.id = c.folder_id
             WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            Ok((
                PathBuf::from(&key).join(name),
                Capture {
                    taken: r.get(1)?,
                    camera: r.get(2)?,
                    lens: r.get(3)?,
                    iso: r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                    shutter: r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    aperture: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                    focal_len: r.get::<_, Option<f64>>(7)?.map(|v| v as f32),
                },
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;

    #[test]
    fn a_capture_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let c = Capture {
            taken: Some(1_779_046_001),
            camera: Some("FUJIFILM X-T4".into()),
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(1600),
            shutter: Some(0.02),
            aperture: Some(2.0),
            focal_len: Some(23.0),
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
    }

    #[test]
    fn an_empty_capture_is_still_recorded() {
        // ⚠⚠ A file with no EXIF must be *marked as read*, or the background pass re-reads it on
        // every folder open forever — 11–15 ms of file reading each time, for nothing.
        //
        // ⚠ `Some(default)` and `None` mean different things here: read-and-had-nothing versus
        // never-read.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), None);
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default())
            .unwrap();
        assert_eq!(
            cat.capture_of(Path::new("/a/one.RAF")).unwrap(),
            Some(Capture::default())
        );
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["two.RAF".to_string()],
            "a file recorded as having no EXIF must not be queued again"
        );
    }

    #[test]
    fn a_description_round_trips_and_an_unknown_photo_is_none() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        assert_eq!(cat.description_of(Path::new("/a/one.RAF")).unwrap(), None);
        let d = Description {
            title: "Harbour at dusk".into(),
            caption: "Fishing boats returning, Whitby.".into(),
            copyright: "© 2026 W".into(),
            creator: "W".into(),
        };
        cat.set_description(Path::new("/a/one.RAF"), &d).unwrap();
        assert_eq!(
            cat.description_of(Path::new("/a/one.RAF")).unwrap(),
            Some(d)
        );
        // A photo the catalog has never seen is `None`, not an error.
        assert_eq!(cat.description_of(Path::new("/a/ghost.RAF")).unwrap(), None);
    }

    #[test]
    fn two_copies_of_one_file_share_a_description() {
        // ⚠ Per **file**, like keywords and capture: a title says what is in the picture, which
        // every copy shares. Only marks describe the image you made. Spec §1.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        let d = Description {
            title: "Shared".into(),
            ..Description::default()
        };
        cat.set_description(Path::new("/a/one.RAF"), &d).unwrap();
        // There is no copy argument to ask with; the one row serves both.
        assert_eq!(
            cat.description_of(Path::new("/a/one.RAF")).unwrap(),
            Some(d)
        );
    }

    #[test]
    fn an_all_empty_description_removes_the_row() {
        // ⚠ "Absent" and "blank" are one state, so the table holds only photos that say
        // something — and a reader cannot mistake four empty strings for a deliberate
        // assertion of no rights, the blank-copyright problem `ExportMetadata` documents.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_description(
            Path::new("/a/one.RAF"),
            &Description {
                caption: "gone soon".into(),
                ..Description::default()
            },
        )
        .unwrap();
        cat.set_description(
            Path::new("/a/one.RAF"),
            &Description {
                caption: "   ".into(),
                ..Description::default()
            },
        )
        .unwrap();
        assert_eq!(cat.description_of(Path::new("/a/one.RAF")).unwrap(), None);
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_description", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "a blank row survived");
    }

    #[test]
    fn capture_is_per_file_so_every_copy_shares_it() {
        // ⚠⚠ Per *file*, like `photo_keywords` and unlike marks: capture metadata describes the
        // **exposure**, which every copy of a file shares. A rating describes the image you made,
        // which they do not.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 2).unwrap();
        // ⚠ Three photo rows, and the queue must name the file **once** — without DISTINCT it
        // would be read three times.
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["one.RAF".to_string()]
        );
        let c = Capture {
            iso: Some(400),
            ..Default::default()
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
        assert!(cat
            .files_without_capture(Path::new("/a"))
            .unwrap()
            .is_empty());
    }
}
