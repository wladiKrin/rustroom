//! Stacks made by hand (M10.8c): which stack a file belongs to, and whether it is the one on top.
//!
//! ⚠ Per **file**, keyed like every per-file table — `(folder, file)` — and one of `PER_FILE_TABLES`, so a
//! move carries it. A stack never spans folders (Lightroom's rule), which the key makes structural: the
//! app keys a hand stack on `(folder, id)`.

use super::{split, Catalog, CatalogError};
use std::path::{Path, PathBuf};

/// A file's place in a hand stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandMember {
    /// The stack it belongs to.
    pub stack: i64,
    /// Its place in the stack — the lowest is the top, the photo a collapsed stack shows (v12; v11
    /// carried only a `top` flag, migrated as order 0).
    pub ord: i64,
    /// Made by *Stack by capture time* — which that menu may take apart or replace; ⌘G's are never
    /// touched by it (the author's choice, M10.8c after use).
    pub by_time: bool,
}

impl Catalog {
    /// Every file in `folder` that belongs to a hand stack, with its place in it.
    pub fn hand_stacks_in(
        &self,
        folder: &Path,
    ) -> Result<Vec<(PathBuf, HandMember)>, CatalogError> {
        let dir = folder.to_string_lossy().to_string();
        let mut st = self.conn.prepare(
            "SELECT s.file_name, s.stack_id, s.ord, s.by_time FROM photo_stacks s
             JOIN folders f ON f.id = s.folder_id WHERE f.path = ?1",
        )?;
        let rows = st
            .query_map([&dir], |r| {
                Ok((
                    folder.join(r.get::<_, String>(0)?),
                    HandMember {
                        stack: r.get(1)?,
                        ord: r.get(2)?,
                        by_time: r.get::<_, i64>(3)? != 0,
                    },
                ))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    /// Put `photo` in a hand stack (`Some`) or take it out (`None`). A photo whose folder the catalog
    /// does not know is a quiet no-op — the catalog is not authoritative.
    pub fn set_hand_stack(
        &self,
        photo: &Path,
        member: Option<HandMember>,
    ) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        match member {
            Some(m) => self.conn.execute(
                "INSERT INTO photo_stacks (folder_id, file_name, stack_id, ord, by_time)
                 SELECT id, ?2, ?3, ?4, ?5 FROM folders WHERE path = ?1
                 ON CONFLICT(folder_id, file_name) DO UPDATE SET stack_id = ?3, ord = ?4, by_time = ?5",
                rusqlite::params![folder, name, m.stack, m.ord, m.by_time as i64],
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
    use crate::db::{Catalog, HandMember};
    use std::path::{Path, PathBuf};

    fn m(stack: i64, ord: i64) -> Option<HandMember> {
        Some(HandMember {
            stack,
            ord,
            by_time: false,
        })
    }

    #[test]
    fn a_hand_stack_round_trips_and_comes_apart() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        assert_eq!(cat.next_stack_id().unwrap(), 1);
        cat.set_hand_stack(Path::new("/x/a.RAF"), m(1, 0)).unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), m(1, 1)).unwrap();
        let mut got = cat.hand_stacks_in(Path::new("/x")).unwrap();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                (PathBuf::from("/x/a.RAF"), m(1, 0).unwrap()),
                (PathBuf::from("/x/b.RAF"), m(1, 1).unwrap())
            ]
        );
        assert_eq!(cat.next_stack_id().unwrap(), 2);
        cat.set_hand_stack(Path::new("/x/a.RAF"), m(1, 1)).unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), None).unwrap();
        assert_eq!(
            cat.hand_stacks_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), m(1, 1).unwrap())],
            "an upsert replaces, a None removes"
        );
    }

    /// ⚠ One of `PER_FILE_TABLES`: Rustroom's own move carries the membership.
    #[test]
    fn a_move_carries_the_membership() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.set_hand_stack(Path::new("/x/a.RAF"), m(4, 0)).unwrap();
        cat.move_photo(Path::new("/x/a.RAF"), Path::new("/y/c.RAF"))
            .unwrap();
        assert!(cat.hand_stacks_in(Path::new("/x")).unwrap().is_empty());
        assert_eq!(
            cat.hand_stacks_in(Path::new("/y")).unwrap(),
            vec![(PathBuf::from("/y/c.RAF"), m(4, 0).unwrap())]
        );
    }

    /// Forgetting a folder and undoing it brings the stacks back.
    #[test]
    fn forgetting_a_folder_and_undoing_it_keeps_its_stacks() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.set_hand_stack(Path::new("/x/a.RAF"), m(2, 0)).unwrap();
        cat.set_hand_stack(Path::new("/x/b.RAF"), m(2, 1)).unwrap();
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
