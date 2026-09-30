//! A photo's marks — flag, rating, colour label — and the XMP stamp that records when they were last written out.

use super::{Catalog, CatalogError};
use std::path::{Path, PathBuf};

/// Pick / reject, or neither.
///
/// ⚠ Stored as `1 / -1 / 0` rather than as text: the neutral value is then `0`, the same as an
/// unrated star and an unset label, so "no mark" needs no special case anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    fn to_i64(self) -> i64 {
        match self {
            Flag::None => 0,
            Flag::Pick => 1,
            Flag::Reject => -1,
        }
    }

    fn from_i64(v: i64) -> Self {
        match v {
            1 => Flag::Pick,
            -1 => Flag::Reject,
            // ⚠ Anything else reads as unflagged rather than erroring. The column is an integer a
            // future version could widen, and a catalog is never a reason to fail.
            _ => Flag::None,
        }
    }
}

/// Everything a photo is marked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marks {
    /// 0..=5, where 0 is unrated.
    pub rating: u8,
    pub flag: Flag,
    /// 0 is no label; 1..=5 are the colours.
    pub label: u8,
}

impl Marks {
    pub const MAX_RATING: u8 = 5;
    /// ⚠ Four, not five: keys `6`–`9` are what set a label, so a fifth colour would be
    /// unreachable — and an unreachable value is a bug nobody can see.
    pub const MAX_LABEL: u8 = 4;

    /// Whether anything is set.
    pub fn is_empty(self) -> bool {
        self == Marks::default()
    }

    /// ⚠ Clamped on the way in, not trusted. The value arrives from a key handler and one day
    /// from an import; storing 9 stars would render as garbage in every badge assuming 0..=5.
    fn clamped(self) -> Self {
        Self {
            rating: self.rating.min(Self::MAX_RATING),
            flag: self.flag,
            label: self.label.min(Self::MAX_LABEL),
        }
    }
}

impl Catalog {
    /// What a photo is marked with. Unknown photos read as unmarked.
    pub fn marks(&self, photo: &Path, copy: u32) -> Result<Marks, CatalogError> {
        let Some(id) = self.photo_id(photo, copy)? else {
            return Ok(Marks::default());
        };
        let (rating, flag, label): (i64, i64, i64) = self.conn.query_row(
            "SELECT rating, flag, label FROM photos WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(Marks {
            rating: rating.clamp(0, Marks::MAX_RATING as i64) as u8,
            flag: Flag::from_i64(flag),
            label: label.clamp(0, Marks::MAX_LABEL as i64) as u8,
        })
    }

    /// Mark a photo. ⚠ A photo the catalog does not know is silently skipped — the catalog is
    /// never a reason an action fails.
    pub fn set_marks(&self, photo: &Path, copy: u32, marks: Marks) -> Result<(), CatalogError> {
        let Some(id) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        let m = marks.clamped();
        self.conn.execute(
            "UPDATE photos SET rating = ?2, flag = ?3, label = ?4 WHERE id = ?1",
            rusqlite::params![id, m.rating as i64, m.flag.to_i64(), m.label as i64],
        )?;
        Ok(())
    }

    /// Every *marked* photo in a folder, as `(path, copy, marks)`.
    ///
    /// ⚠⚠ One query per rebuild, never one per grid cell: the grid draws sixty times a second
    /// over hundreds of thumbnails, and a query per cell would be a database round trip per photo
    /// per frame. Only marked rows come back, so an unmarked folder costs almost nothing.
    pub fn marks_in(&self, folder: &Path) -> Result<Vec<(PathBuf, u32, Marks)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name, p.copy, p.rating, p.flag, p.label
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND (p.rating != 0 OR p.flag != 0 OR p.label != 0)",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            let copy: u32 = r.get(1)?;
            let rating: i64 = r.get(2)?;
            let flag: i64 = r.get(3)?;
            let label: i64 = r.get(4)?;
            Ok((
                PathBuf::from(&key).join(name),
                copy,
                Marks {
                    rating: rating.clamp(0, Marks::MAX_RATING as i64) as u8,
                    flag: Flag::from_i64(flag),
                    label: label.clamp(0, Marks::MAX_LABEL as i64) as u8,
                },
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// When each photo's XMP sidecar was last imported or written, for a whole folder.
    ///
    /// ⚠⚠ **Copy 0 only, and one query per folder.** XMP has no notion of a virtual copy, so
    /// only the original's row carries the timestamp — and the grid rule applies here too: this
    /// is called once per folder open, never once per photo.
    ///
    /// ⚠ Rows with `0` are returned like any other. `0` means "never seen", which is exactly
    /// what the caller compares against, so filtering them out would hide the case that matters
    /// most: a folder rated in Lightroom that Rustroom has never opened.
    pub fn xmp_mtimes_in(&self, folder: &Path) -> Result<Vec<(PathBuf, i64)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name, p.xmp_mtime
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND p.copy = 0",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            let secs: i64 = r.get(1)?;
            Ok((PathBuf::from(&key).join(name), secs))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Record that this photo's sidecar, as of `secs`, is one Rustroom has seen.
    ///
    /// ⚠ Copy 0, because that is the only row an XMP sidecar describes. ⚠ A photo the catalog
    /// does not know is silently skipped — `set_marks`' rule, for its reason: the catalog is
    /// never why an action fails.
    pub fn set_xmp_mtime(&self, photo: &Path, secs: i64) -> Result<(), CatalogError> {
        let Some(id) = self.photo_id(photo, 0)? else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE photos SET xmp_mtime = ?2 WHERE id = ?1",
            rusqlite::params![id, secs],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entry;

    #[test]
    fn marks_round_trip() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let want = Marks {
            rating: 4,
            flag: Flag::Pick,
            label: 2,
        };
        cat.set_marks(Path::new("/x/a.RAF"), 0, want).unwrap();
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap(), want);
    }

    #[test]
    fn copies_of_one_file_are_marked_separately() {
        // ⚠⚠ The decision this feature was designed around: a rating describes **the image you
        // made**, so a black-and-white copy can be rated apart from its colour original. That is
        // why copies exist. If this failed, per-copy storage was pointless.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();

        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 1,
                ..Marks::default()
            },
        )
        .unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            1,
            Marks {
                rating: 5,
                ..Marks::default()
            },
        )
        .unwrap();

        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating, 1);
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 1).unwrap().rating, 5);
    }

    #[test]
    fn marks_survive_a_rename_and_a_renumbering() {
        // ⚠⚠ Marks live on the row, and plan 1 made renames and `Reindex` move the row. This
        // asserts that end to end — it is what stops a cull being undone by housekeeping.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 3).unwrap();
        let want = Marks {
            rating: 5,
            flag: Flag::Pick,
            label: 1,
        };
        cat.set_marks(Path::new("/x/a.RAF"), 3, want).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();
        cat.renumber_copy(Path::new("/x/b.RAF"), 3, 1).unwrap();

        assert_eq!(cat.marks(Path::new("/x/b.RAF"), 1).unwrap(), want);
    }

    #[test]
    fn an_out_of_range_rating_is_clamped_rather_than_stored() {
        // ⚠ The value reaches here from a key handler and a future import. Storing 9 stars would
        // render as garbage in every badge that assumes 0..=5.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 9,
                ..Marks::default()
            },
        )
        .unwrap();
        // ⚠⚠ Asserted on what is **stored**, not on what `marks` returns. `marks` clamps on the
        // read path too, so reading it back would pass with the write clamp deleted — sabotage
        // said so. Defence in depth is fine; a test that cannot see the layer it names is not.
        // A stored 9 would also be seen by any future `WHERE rating >= 3` query.
        let stored: i64 = cat
            .conn
            .query_row("SELECT rating FROM photos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored, 5,
            "the out-of-range value must not reach the column"
        );
    }

    #[test]
    fn marks_of_a_folder_come_back_in_one_query() {
        // ⚠ The grid draws sixty times a second. One query per rebuild, never one per cell.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 3,
                ..Marks::default()
            },
        )
        .unwrap();

        let got = cat.marks_in(Path::new("/x")).unwrap();
        assert_eq!(got.len(), 1, "only marked photos are returned");
        assert_eq!(got[0].0, PathBuf::from("/x/a.RAF"));
        assert_eq!(got[0].1, 0);
        assert_eq!(got[0].2.rating, 3);
    }

    #[test]
    fn marking_a_photo_the_catalog_does_not_know_is_harmless() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.set_marks(
            Path::new("/ghost/x.RAF"),
            0,
            Marks {
                rating: 3,
                ..Marks::default()
            },
        )
        .expect("must not error");
    }

    #[test]
    fn an_xmp_mtime_round_trips_and_defaults_to_zero() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 0)],
            "a fresh row has never been seen"
        );
        cat.set_xmp_mtime(Path::new("/x/a.RAF"), 1_700_000_000)
            .unwrap();
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 1_700_000_000)]
        );
    }

    #[test]
    fn xmp_mtimes_come_back_for_copy_zero_only() {
        // ⚠ XMP has no notion of a virtual copy, so only the original carries the timestamp.
        // Without the `copy = 0` clause a three-copy photo would come back three times and the
        // caller's map would be built from whichever row happened to be last.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 2).unwrap();
        cat.set_xmp_mtime(Path::new("/x/a.RAF"), 42).unwrap();

        let rows = cat.xmp_mtimes_in(Path::new("/x")).unwrap();
        assert_eq!(rows, vec![(PathBuf::from("/x/a.RAF"), 42)]);
    }

    #[test]
    fn setting_the_xmp_mtime_of_an_unknown_photo_is_harmless() {
        let cat = Catalog::open_in_memory().unwrap();
        assert!(cat.set_xmp_mtime(Path::new("/nowhere/a.RAF"), 7).is_ok());
    }
}
