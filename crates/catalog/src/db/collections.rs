//! Collections: named sets of photos, across folders.

use super::{unix_now, Catalog, CatalogError};
use crate::snapshot::CatalogSnapshot;
use std::path::{Path, PathBuf};

/// A collection, with how many photos are in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

impl Catalog {
    /// Create `name`, or return the existing collection of that name.
    ///
    /// ⚠ A duplicate is **not** an error. The name is the identity a user thinks in, and failing
    /// the second time "Portfolio" is used would be a bug rather than a safeguard.
    pub fn create_collection(&self, name: &str) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO collections (name, created) VALUES (?1, ?2)
             ON CONFLICT(name) DO NOTHING",
            rusqlite::params![name, unix_now()],
        )?;
        Ok(self
            .conn
            .query_row("SELECT id FROM collections WHERE name = ?1", [name], |r| {
                r.get(0)
            })?)
    }

    /// Every collection, name-ordered, with its size.
    ///
    /// ⚠ Ordered in SQL and **case-insensitively**, matching the browser's own rule. A list that
    /// reshuffles between frames is the sparse-index class of bug this project has hit three
    /// times in the Library.
    pub fn collections(&self) -> Result<Vec<CollectionRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, (SELECT count(*) FROM collection_photos cp
                                   WHERE cp.collection_id = c.id)
             FROM collections c
             ORDER BY lower(c.name), c.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CollectionRow {
                id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Remove a collection. ⚠ Its memberships go with it; **the photos do not.**
    pub fn delete_collection(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let (name, created) = tx.query_row(
            "SELECT name, created FROM collections WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut st =
            tx.prepare("SELECT photo_id, added FROM collection_photos WHERE collection_id = ?1")?;
        let members: Vec<(i64, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);
        tx.execute("DELETE FROM collections WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Collection {
            name,
            created,
            members,
        })
    }

    /// Put one version of one file into a collection.
    ///
    /// ⚠ Idempotent, because adding twice is the normal case — a user re-selects and re-adds.
    ///
    /// ⚠ A photo the catalog does not know is silently skipped: the catalog is not authoritative
    /// and must never be a reason an action fails.
    ///
    /// Returns whether a row changed — `false` for a photo already in, or one the catalog does not
    /// know — so an undo can record only what this call did.
    pub fn add_to_collection(
        &self,
        collection: i64,
        photo: &Path,
        copy: u32,
    ) -> Result<bool, CatalogError> {
        let Some(photo_id) = self.photo_id(photo, copy)? else {
            return Ok(false);
        };
        Ok(self.conn.execute(
            "INSERT INTO collection_photos (collection_id, photo_id, added) VALUES (?1, ?2, ?3)
             ON CONFLICT(collection_id, photo_id) DO NOTHING",
            rusqlite::params![collection, photo_id, unix_now()],
        )? > 0)
    }

    /// Take a copy out of a collection; whether a row changed, as [`Self::add_to_collection`].
    pub fn remove_from_collection(
        &self,
        collection: i64,
        photo: &Path,
        copy: u32,
    ) -> Result<bool, CatalogError> {
        let Some(photo_id) = self.photo_id(photo, copy)? else {
            return Ok(false);
        };
        Ok(self.conn.execute(
            "DELETE FROM collection_photos WHERE collection_id = ?1 AND photo_id = ?2",
            rusqlite::params![collection, photo_id],
        )? > 0)
    }

    /// Everything in a collection, as `(path, copy)` — **across every folder**.
    ///
    /// ⚠⚠ Cross-folder is the whole reason the catalog is one database rather than one per
    /// folder. If this could not span folders, that decision was wrong.
    pub fn collection_members(&self, collection: i64) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy
             FROM collection_photos cp
             JOIN photos p  ON p.id = cp.photo_id
             JOIN folders f ON f.id = p.folder_id
             WHERE cp.collection_id = ?1
             ORDER BY lower(f.path), lower(p.file_name), p.copy",
        )?;
        let rows = stmt.query_map([collection], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            let copy: u32 = r.get(2)?;
            Ok((PathBuf::from(folder).join(name), copy))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entry;

    #[test]
    fn a_collection_can_be_created_and_listed() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_collection("Portfolio").unwrap();
        let got = cat.collections().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert_eq!(got[0].name, "Portfolio");
        assert_eq!(got[0].count, 0, "a new collection is empty");
    }

    #[test]
    fn collections_come_back_in_name_order() {
        // ⚠ Ordered in SQL, not by the caller. A list that reshuffles between frames is the
        // sparse-index class of bug this project has hit three times in the Library.
        let cat = Catalog::open_in_memory().unwrap();
        for n in ["Zebra", "apple", "Middle"] {
            cat.create_collection(n).unwrap();
        }
        let got: Vec<String> = cat
            .collections()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            got,
            ["apple", "Middle", "Zebra"],
            "case-insensitive, like the browser"
        );
    }

    #[test]
    fn creating_a_collection_that_already_exists_returns_the_same_one() {
        // ⚠ Not an error: the name is the identity a user thinks in, and failing on a duplicate
        // would make "add to Portfolio" fail the second time it is used.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_collection("Portfolio").unwrap();
        let b = cat.create_collection("Portfolio").unwrap();
        assert_eq!(a, b);
        assert_eq!(cat.collections().unwrap().len(), 1);
    }

    #[test]
    fn adding_a_photo_to_a_collection_counts_it_once() {
        // ⚠ Adding twice is the normal case — a user re-selects and re-adds. The PRIMARY KEY on
        // (collection, photo) is what makes it idempotent, asserted through the API rather than
        // merely declared in the schema.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();

        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        assert_eq!(cat.collections().unwrap()[0].count, 1);
    }

    #[test]
    fn a_collection_holds_photos_from_several_folders() {
        // ⚠⚠ The whole reason the catalog is one database rather than one per folder. If this
        // could not work, the design decision that chose a single catalog was wrong.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.sync_folder(Path::new("/y"), &[entry("b.RAF")]).unwrap();
        let c = cat.create_collection("Best").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
        cat.add_to_collection(c, Path::new("/y/b.RAF"), 0).unwrap();

        let mut members = cat.collection_members(c).unwrap();
        members.sort();
        assert_eq!(
            members,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/y/b.RAF"), 0)
            ]
        );
    }

    #[test]
    fn removing_a_photo_leaves_the_collection_and_the_photo() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.remove_from_collection(c, Path::new("/x/a.RAF"), 0)
            .unwrap();

        assert_eq!(
            cat.collections().unwrap().len(),
            1,
            "the collection survives"
        );
        assert_eq!(cat.collections().unwrap()[0].count, 0);
        assert_eq!(cat.photo_count().unwrap(), 1, "and so does the photo");
    }

    #[test]
    fn deleting_a_collection_never_touches_the_photos() {
        // ⚠⚠ The guard people actually worry about. `ON DELETE CASCADE` removes the *memberships*
        // and must stop there — a collection is a view onto photos, not a container of them.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.delete_collection(c).unwrap();

        assert!(cat.collections().unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 2, "the photos are untouched");
    }

    #[test]
    fn a_renamed_file_stays_in_its_collections() {
        // ⚠⚠ The integration plan 1 built `move_photo` for, asserted end to end. Membership is
        // by row id, so moving the row carries the membership — which is the reason a rename must
        // update the catalog in the same operation as the file move.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.move_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(
            cat.collection_members(c).unwrap(),
            vec![(PathBuf::from("/x/b.RAF"), 0)],
            "the membership must follow the rename"
        );
    }

    #[test]
    fn adding_a_photo_the_catalog_does_not_know_is_harmless() {
        // ⚠ The catalog is not authoritative, so nothing may fail because of it.
        let cat = Catalog::open_in_memory().unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/ghost/x.RAF"), 0)
            .expect("must not error");
        assert_eq!(cat.collections().unwrap()[0].count, 0);
    }

    /// Adding and removing report whether they changed a row (2026-10-01) — what lets an undo of
    /// "add" leave a photo that was already in the collection.
    #[test]
    fn adding_and_removing_report_a_change() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        let a = Path::new("/x/a.RAF");
        assert!(cat.add_to_collection(c, a, 0).unwrap());
        assert!(!cat.add_to_collection(c, a, 0).unwrap(), "already in");
        assert!(cat.remove_from_collection(c, a, 0).unwrap());
        assert!(!cat.remove_from_collection(c, a, 0).unwrap(), "already out");
        assert!(
            !cat.add_to_collection(c, Path::new("/x/unknown.RAF"), 0)
                .unwrap(),
            "a photo the catalog does not know changes nothing"
        );
    }
}
