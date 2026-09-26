//! A photo's undo history (M9.7): one opaque text body per `(folder, file, copy)`.
//!
//! ⚠⚠ **Why the catalog and not the sidecar** — the decision M9.7's done-when asks to be written
//! down. The sidecar is the photograph's *description*, read by other tools, backups and people,
//! and rewritten on every save; a history is *working state*. Measured sidecars are 3.4 KB median
//! and 44 KB at most (brush masks), and every undo step is a whole `Edits`, so a hundred steps would
//! grow them to 340 KB–4 MB. The accepted cost: a folder moved outside Rustroom, or a deleted
//! catalog, loses the history — never the edits. Lightroom makes the same choice.
//!
//! ⚠ The body is opaque on purpose: this crate knows nothing of `Edits`. `rustroom_app::
//! history_store` encodes and decodes it.
//!
//! ⚠ `forget_folder` cascades these rows away and its undo does not bring them back: a history is
//! working state, and a forgotten folder has none worth restoring.

use super::{split, Catalog, CatalogError};
use std::path::Path;

impl Catalog {
    /// Store `body` as the history of `photo`'s copy `copy`, replacing any before it.
    ///
    /// ⚠ A quiet no-op when the photo's folder is not in the catalog — the `set_description`
    /// rule: there is no row to hang it on, and the history is not worth an error.
    pub fn save_history(&self, photo: &Path, copy: u32, body: &str) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO photo_history (folder_id, file_name, copy, body)
             SELECT id, ?2, ?3, ?4 FROM folders WHERE path = ?1
             ON CONFLICT(folder_id, file_name, copy) DO UPDATE SET body = ?4",
            rusqlite::params![folder, name, copy, body],
        )?;
        Ok(())
    }

    /// The stored history of `photo`'s copy `copy`, if any.
    pub fn load_history(&self, photo: &Path, copy: u32) -> Result<Option<String>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let mut st = self.conn.prepare(
            "SELECT h.body FROM photo_history h JOIN folders f ON f.id = h.folder_id
             WHERE f.path = ?1 AND h.file_name = ?2 AND h.copy = ?3",
        )?;
        let mut rows = st.query(rusqlite::params![folder, name, copy])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// Drop the history of `photo`'s copy `copy`.
    pub fn delete_history(&self, photo: &Path, copy: u32) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "DELETE FROM photo_history
             WHERE file_name = ?2 AND copy = ?3
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![folder, name, copy],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Catalog;
    use crate::CatalogEntry;
    use std::path::{Path, PathBuf};

    fn catalog_with(folder: &str, names: &[&str]) -> Catalog {
        let c = Catalog::open_in_memory().unwrap();
        let entries: Vec<CatalogEntry> = names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(folder).join(n),
                file_name: n.to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        c.sync_folder(Path::new(folder), &entries).unwrap();
        c
    }

    #[test]
    fn a_history_round_trips_and_is_replaced() {
        let c = catalog_with("/p", &["a.raf"]);
        let a = Path::new("/p/a.raf");
        assert_eq!(c.load_history(a, 0).unwrap(), None);
        c.save_history(a, 0, "one").unwrap();
        assert_eq!(c.load_history(a, 0).unwrap().as_deref(), Some("one"));
        c.save_history(a, 0, "two").unwrap();
        assert_eq!(c.load_history(a, 0).unwrap().as_deref(), Some("two"));
        c.delete_history(a, 0).unwrap();
        assert_eq!(c.load_history(a, 0).unwrap(), None);
    }

    /// ⚠ Both directions: a copy's history is its own, and the original's is not disturbed by it.
    #[test]
    fn a_copy_has_its_own_history_and_so_does_another_file() {
        let c = catalog_with("/p", &["a.raf", "b.raf"]);
        let (a, b) = (Path::new("/p/a.raf"), Path::new("/p/b.raf"));
        c.save_history(a, 0, "original").unwrap();
        c.save_history(a, 2, "copy 2").unwrap();
        c.save_history(b, 0, "b").unwrap();
        assert_eq!(c.load_history(a, 0).unwrap().as_deref(), Some("original"));
        assert_eq!(c.load_history(a, 2).unwrap().as_deref(), Some("copy 2"));
        assert_eq!(c.load_history(b, 0).unwrap().as_deref(), Some("b"));
        c.delete_history(a, 2).unwrap();
        assert_eq!(c.load_history(a, 0).unwrap().as_deref(), Some("original"));
    }

    #[test]
    fn a_photo_outside_the_catalog_is_a_quiet_no_op() {
        let c = catalog_with("/p", &["a.raf"]);
        let elsewhere = Path::new("/q/a.raf");
        c.save_history(elsewhere, 0, "x").unwrap();
        assert_eq!(c.load_history(elsewhere, 0).unwrap(), None);
    }

    #[test]
    fn forgetting_a_folder_takes_its_histories() {
        let c = catalog_with("/p", &["a.raf"]);
        let a = Path::new("/p/a.raf");
        c.save_history(a, 0, "x").unwrap();
        let id = c.folders().unwrap()[0].id;
        c.forget_folder(id).unwrap();
        assert_eq!(c.load_history(a, 0).unwrap(), None);
        let n: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM photo_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "the cascade must remove the row, not merely hide it");
    }
}
