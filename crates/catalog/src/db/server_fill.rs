//! Filling a photo's marks from the server it came from — once, and only where it has none (M10.15c).
//!
//! ⚠⚠ **The whole rule is in the SQL**: each update names the empty value and the unfilled column in
//! its `WHERE`, so a mark the user set, or one filled before, is never touched — no read-then-write to
//! race, and a fill that changes nothing changes no row (`change_count` stays).

use super::{split, Catalog, CatalogError, Flag};
use std::path::PathBuf;

/// What the server says about one photo: stars 1–5, and a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFill {
    pub photo: PathBuf,
    pub rating: Option<u8>,
    pub flag: Option<Flag>,
}

impl Catalog {
    /// Fill each photo's rating and flag from `fills` where the original (copy 0) has none and never
    /// had one filled. Returns how many marks were filled. ⚠ An unknown photo is skipped.
    pub fn fill_from_server(&self, fills: &[ServerFill]) -> Result<usize, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let row =
            "folder_id = (SELECT id FROM folders WHERE path = ?1) AND file_name = ?2 AND copy = 0";
        let mut n = 0;
        for f in fills {
            let Some((folder, name)) = split(&f.photo) else {
                continue;
            };
            if let Some(r) = f.rating.filter(|r| (1..=5).contains(r)) {
                n += tx.execute(
                    &format!(
                        "UPDATE photos SET rating = ?3, filled_rating = 1
                         WHERE {row} AND rating = 0 AND filled_rating = 0"
                    ),
                    rusqlite::params![folder, name, r as i64],
                )?;
            }
            if let Some(flag) = f.flag.filter(|f| *f != Flag::None) {
                n += tx.execute(
                    &format!(
                        "UPDATE photos SET flag = ?3, filled_flag = 1
                         WHERE {row} AND flag = 0 AND filled_flag = 0"
                    ),
                    rusqlite::params![folder, name, flag.to_i64()],
                )?;
            }
        }
        tx.commit()?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;
    use crate::db::Marks;
    use std::path::Path;

    fn cat_with(names: &[&str]) -> Catalog {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/i"), &entries_in("/i", names))
            .unwrap();
        cat
    }

    fn fill(name: &str, rating: Option<u8>, flag: Option<Flag>) -> ServerFill {
        ServerFill {
            photo: Path::new("/i").join(name),
            rating,
            flag,
        }
    }

    fn marks(cat: &Catalog, name: &str, copy: u32) -> Marks {
        cat.marks(&Path::new("/i").join(name), copy).unwrap()
    }

    #[test]
    fn an_empty_mark_fills() {
        let cat = cat_with(&["a.jpg"]);
        let n = cat
            .fill_from_server(&[fill("a.jpg", Some(3), Some(Flag::Pick))])
            .unwrap();
        assert_eq!(n, 2);
        let m = marks(&cat, "a.jpg", 0);
        assert_eq!((m.rating, m.flag), (3, Flag::Pick));
    }

    #[test]
    fn a_set_mark_is_never_overridden() {
        let cat = cat_with(&["a.jpg"]);
        let mine = Marks {
            rating: 2,
            flag: Flag::Reject,
            label: 1,
        };
        cat.set_marks(Path::new("/i/a.jpg"), 0, mine).unwrap();
        let n = cat
            .fill_from_server(&[fill("a.jpg", Some(5), Some(Flag::Pick))])
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(marks(&cat, "a.jpg", 0), mine);
    }

    #[test]
    fn a_filled_then_cleared_mark_stays_cleared() {
        let cat = cat_with(&["a.jpg"]);
        let f = [fill("a.jpg", Some(4), Some(Flag::Pick))];
        assert_eq!(cat.fill_from_server(&f).unwrap(), 2, "fixture: it filled");
        cat.set_marks(Path::new("/i/a.jpg"), 0, Marks::default())
            .unwrap();
        assert_eq!(cat.fill_from_server(&f).unwrap(), 0);
        assert_eq!(marks(&cat, "a.jpg", 0), Marks::default());
    }

    #[test]
    fn a_never_filled_photo_rated_later_fills() {
        let cat = cat_with(&["a.jpg"]);
        assert_eq!(
            cat.fill_from_server(&[fill("a.jpg", None, None)]).unwrap(),
            0
        );
        assert_eq!(
            cat.fill_from_server(&[fill("a.jpg", Some(5), None)])
                .unwrap(),
            1
        );
        assert_eq!(marks(&cat, "a.jpg", 0).rating, 5);
    }

    #[test]
    fn only_the_original_is_filled() {
        let cat = cat_with(&["a.jpg"]);
        cat.add_copy(Path::new("/i/a.jpg"), 1).unwrap();
        cat.fill_from_server(&[fill("a.jpg", Some(3), Some(Flag::Pick))])
            .unwrap();
        assert_eq!(
            marks(&cat, "a.jpg", 1),
            Marks::default(),
            "a copy was filled"
        );
        assert_eq!(marks(&cat, "a.jpg", 0).rating, 3);
    }

    #[test]
    fn a_fill_that_changes_nothing_moves_no_counter() {
        let cat = cat_with(&["a.jpg"]);
        let f = [fill("a.jpg", Some(3), Some(Flag::Pick))];
        cat.fill_from_server(&f).unwrap();
        let before = cat.change_count();
        assert_eq!(cat.fill_from_server(&f).unwrap(), 0);
        assert_eq!(cat.change_count(), before, "a no-op fill moved the counter");
    }

    #[test]
    fn an_unknown_photo_is_skipped() {
        let cat = cat_with(&["a.jpg"]);
        assert_eq!(
            cat.fill_from_server(&[fill("absent.jpg", Some(3), None)])
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_moved_photo_keeps_what_was_filled() {
        let cat = cat_with(&["a.jpg"]);
        cat.sync_folder(Path::new("/j"), &[]).unwrap();
        cat.fill_from_server(&[fill("a.jpg", Some(4), None)])
            .unwrap();
        cat.move_photo(Path::new("/i/a.jpg"), Path::new("/j/a.jpg"))
            .unwrap();
        cat.set_marks(Path::new("/j/a.jpg"), 0, Marks::default())
            .unwrap();
        let again = ServerFill {
            photo: Path::new("/j/a.jpg").to_path_buf(),
            rating: Some(4),
            flag: None,
        };
        assert_eq!(cat.fill_from_server(&[again]).unwrap(), 0);
        assert_eq!(
            cat.marks(Path::new("/j/a.jpg"), 0).unwrap().rating,
            0,
            "the move lost the filled state"
        );
    }
}
