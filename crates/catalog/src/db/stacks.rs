//! Stacks made by hand (M10.8c): which stack a file belongs to, and whether it is the one on top.
//!
//! ⚠ Per **file**, keyed like every per-file table — `(folder, file)` — and one of `PER_FILE_TABLES`, so a
//! move carries it. A stack never spans folders (Lightroom's rule), which the key makes structural: the
//! app keys a hand stack on `(folder, id)`.

use super::{split, Catalog, CatalogError};
use std::path::{Path, PathBuf};

impl Catalog {
    /// Every file in `folder` that belongs to a hand stack: `(path, stack id, top)`.
    pub fn hand_stacks_in(&self, folder: &Path) -> Result<Vec<(PathBuf, i64, bool)>, CatalogError> {
        let dir = folder.to_string_lossy().to_string();
        let mut st = self.conn.prepare(
            "SELECT s.file_name, s.stack_id, s.top FROM photo_stacks s
             JOIN folders f ON f.id = s.folder_id WHERE f.path = ?1",
        )?;
        let rows = st
            .query_map([&dir], |r| {
                Ok((
                    folder.join(r.get::<_, String>(0)?),
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)? != 0,
                ))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    /// Put `photo` in a hand stack (`Some((id, top))`) or take it out (`None`). A photo whose folder
    /// the catalog does not know is a quiet no-op — the catalog is not authoritative.
    pub fn set_hand_stack(
        &self,
        photo: &Path,
        stack: Option<(i64, bool)>,
    ) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        match stack {
            Some((id, top)) => self.conn.execute(
                "INSERT INTO photo_stacks (folder_id, file_name, stack_id, top)
                 SELECT id, ?2, ?3, ?4 FROM folders WHERE path = ?1
                 ON CONFLICT(folder_id, file_name) DO UPDATE SET stack_id = ?3, top = ?4",
                rusqlite::params![folder, name, id, top as i64],
            )?,
            None => self.conn.execute(
                "DELETE FROM photo_stacks
                 WHERE file_name = ?2 AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
                rusqlite::params![folder, name],
            )?,
        };
        Ok(())
    }

    /// An id no hand stack has: one past the highest in use.
    pub fn next_stack_id(&self) -> Result<i64, CatalogError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(stack_id), 0) + 1 FROM photo_stacks",
            [],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::test_support::entry;
    use crate::db::Catalog;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_hand_stack_round_trips_and_comes_apart() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        assert_eq!(cat.next_stack_id().unwrap(), 1);
        cat.set_hand_stack(Path::new("/x/a.RAF"), Some((1, true)))
            .unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), Some((1, false)))
            .unwrap();
        let mut got = cat.hand_stacks_in(Path::new("/x")).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                (PathBuf::from("/x/a.RAF"), 1, true),
                (PathBuf::from("/x/b.RAF"), 1, false)
            ]
        );
        assert_eq!(cat.next_stack_id().unwrap(), 2);
        cat.set_hand_stack(Path::new("/x/a.RAF"), Some((1, false)))
            .unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), None).unwrap();
        assert_eq!(
            cat.hand_stacks_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 1, false)],
            "an upsert replaces, a None removes"
        );
    }

    /// ⚠ One of `PER_FILE_TABLES`: Rustroom's own move carries the membership.
    #[test]
    fn a_move_carries_the_membership() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.set_hand_stack(Path::new("/x/a.RAF"), Some((4, true)))
            .unwrap();
        cat.move_photo(Path::new("/x/a.RAF"), Path::new("/y/c.RAF"))
            .unwrap();
        assert!(cat.hand_stacks_in(Path::new("/x")).unwrap().is_empty());
        assert_eq!(
            cat.hand_stacks_in(Path::new("/y")).unwrap(),
            vec![(PathBuf::from("/y/c.RAF"), 4, true)]
        );
    }

    /// Forgetting a folder and undoing it brings the stacks back.
    #[test]
    fn forgetting_a_folder_and_undoing_it_keeps_its_stacks() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.set_hand_stack(Path::new("/x/a.RAF"), Some((2, true)))
            .unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), Some((2, false)))
            .unwrap();
        let id = cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == Path::new("/x"))
            .unwrap()
            .id;
        let snap = cat.forget_folder(id).unwrap();
        assert!(
            cat.hand_stacks_in(Path::new("/x")).unwrap().is_empty(),
            "the sample"
        );
        cat.restore(&snap).unwrap();
        assert_eq!(cat.hand_stacks_in(Path::new("/x")).unwrap().len(), 2);
    }
}
