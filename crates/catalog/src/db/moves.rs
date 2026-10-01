//! Recognising a photo moved on disk between known folders (M9.9).
//!
//! ⚠ A child module of `db` — `db.rs` is the crate's largest file — reaching the private
//! connection.
//!
//! ⚠⚠ **The rule is strict on purpose, because it runs automatically** (the author's choice): a
//! file new to a folder is the same photo as a row in ANOTHER folder only with the same file name,
//! size AND modification time, and only when that old path is GONE — a surviving original makes
//! the new file a copy, not a move. Exactly one candidate relinks; zero or several leave the file
//! new, as before. ⚠ The catalog checks no filesystem: `gone` is the caller's.

use super::{file_facts, Catalog, CatalogError};
use crate::CatalogEntry;
use std::path::{Path, PathBuf};

impl Catalog {
    /// Relink the files of `entries` (just scanned in `folder`) that are photos moved from another
    /// folder the catalog knows. Returns `(from, to)` for each one relinked. Call BEFORE
    /// `sync_folder`, which would otherwise add them as new rows.
    pub fn relink_moved(
        &self,
        folder: &Path,
        entries: &[CatalogEntry],
        gone: impl Fn(&Path) -> bool,
    ) -> Result<Vec<(PathBuf, PathBuf)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut moved = Vec::new();
        for e in entries {
            let name = e
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let known_here: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM photos p JOIN folders f ON f.id = p.folder_id
                                WHERE f.path = ?1 AND p.file_name = ?2)",
                rusqlite::params![key, name],
                |r| r.get(0),
            )?;
            if known_here {
                continue;
            }
            let (size, mtime) = file_facts(e);
            let (Some(size), Some(mtime)) = (size, mtime) else {
                continue;
            };
            let candidates: Vec<(i64, String)> = {
                let mut st = self.conn.prepare(
                    "SELECT p.folder_id, f.path FROM photos p JOIN folders f ON f.id = p.folder_id
                     WHERE p.file_name = ?1 AND p.copy = 0 AND p.size = ?2 AND p.mtime = ?3
                       AND f.path != ?4",
                )?;
                let rows = st.query_map(rusqlite::params![name, size, mtime, key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
                rows.filter_map(Result::ok)
                    .filter(|(_, dir)| gone(&PathBuf::from(dir).join(&name)))
                    .collect()
            };
            let [(old_folder, old_dir)] = candidates.as_slice() else {
                continue;
            };
            self.relink(*old_folder, &key, &name)?;
            moved.push((PathBuf::from(old_dir).join(&name), e.path.clone()));
        }
        Ok(moved)
    }

    /// Move every row keyed on `(old_folder, name)` to the folder at `new_path`, in one
    /// transaction.
    fn relink(&self, old_folder: i64, new_path: &str, name: &str) -> Result<(), CatalogError> {
        self.move_rows(old_folder, name, new_path, name)
    }

    /// Follow a move Rustroom made itself: `from` is now `to`, in another folder and perhaps under
    /// another name (2026-10-01, Move to folder). Every copy's rows and every per-file table follow;
    /// collection membership rides the photo row's id. A photo the catalog does not know is not an
    /// error — the catalog is not authoritative.
    pub fn move_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        let (Some((from_dir, from_name)), Some((to_dir, to_name))) =
            (super::split(from), super::split(to))
        else {
            return Ok(());
        };
        let old: Option<i64> = self
            .conn
            .query_row("SELECT id FROM folders WHERE path = ?1", [&from_dir], |r| {
                r.get(0)
            })
            .ok();
        match old {
            Some(old) => self.move_rows(old, &from_name, &to_dir, &to_name),
            None => Ok(()),
        }
    }

    /// Whether the catalog has a photo row at `path` — a destination name a move must not take even
    /// when no file is there (a vanished file's rows are kept: absence is not deletion).
    pub fn knows(&self, path: &Path) -> Result<bool, CatalogError> {
        let Some((dir, name)) = super::split(path) else {
            return Ok(false);
        };
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM photos p JOIN folders f ON f.id = p.folder_id
                            WHERE f.path = ?1 AND p.file_name = ?2)",
            rusqlite::params![dir, name],
            |r| r.get(0),
        )?)
    }

    /// Re-key every per-file row from `(old_folder, old_name)` to `(new_path, new_name)`, creating the
    /// destination folder's row if it is new — one transaction.
    fn move_rows(
        &self,
        old_folder: i64,
        old_name: &str,
        new_path: &str,
        new_name: &str,
    ) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO folders (path, last_seen) VALUES (?1, 0) ON CONFLICT(path) DO NOTHING",
            [new_path],
        )?;
        let new_folder: i64 =
            tx.query_row("SELECT id FROM folders WHERE path = ?1", [new_path], |r| {
                r.get(0)
            })?;
        for table in PER_FILE_TABLES {
            tx.execute(
                &format!(
                    "UPDATE {table} SET folder_id = ?1, file_name = ?4
                     WHERE folder_id = ?2 AND file_name = ?3"
                ),
                rusqlite::params![new_folder, old_folder, old_name, new_name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// ⚠⚠ **Every table keyed on `(folder_id, file_name)`** — a new one must be added here, or a move
/// (whether Rustroom made it or only noticed it) would leave its rows behind. One list, shared by
/// [`Catalog::relink_moved`] and [`Catalog::move_photo`], so the two cannot disagree.
const PER_FILE_TABLES: [&str; 6] = [
    "photos",
    "photo_keywords",
    "photo_capture",
    "photo_description",
    "photo_history",
    "photo_stacks",
];

#[cfg(test)]
mod tests {
    use crate::db::{Catalog, Marks};
    use crate::CatalogEntry;
    use std::path::{Path, PathBuf};

    /// A real temp tree: `a/x.RAF` catalogued with marks, a keyword, a copy, a description and a
    /// history; then the file is MOVED to `b/`. Unique per test: pid plus the tag.
    struct World {
        root: PathBuf,
        cat: Catalog,
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn entry(p: &Path) -> CatalogEntry {
        CatalogEntry {
            path: p.to_path_buf(),
            file_name: p.file_name().unwrap().to_string_lossy().to_string(),
            modified: std::fs::metadata(p).unwrap().modified().unwrap(),
            edited: None,
        }
    }

    fn world(tag: &str) -> World {
        let root = test_dir::path(&format!("moves-{tag}"));
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let x = root.join("a/x.RAF");
        std::fs::write(&x, b"some raw bytes").unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(&root.join("a"), &[entry(&x)]).unwrap();
        cat.sync_folder(&root.join("b"), &[]).unwrap();
        cat.add_copy(&x, 1).unwrap();
        cat.set_marks(
            &x,
            0,
            Marks {
                rating: 4,
                ..Marks::default()
            },
        )
        .unwrap();
        let kw = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(kw, &x).unwrap();
        cat.save_history(&x, 0, "history").unwrap();
        cat.set_description(
            &x,
            &crate::db::Description {
                title: "Eiffel".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        cat.set_capture(
            &x,
            &core_image::Capture {
                iso: Some(3200),
                ..Default::default()
            },
        )
        .unwrap();
        World { root, cat }
    }

    fn move_file(w: &World) -> PathBuf {
        let to = w.root.join("b/x.RAF");
        std::fs::rename(w.root.join("a/x.RAF"), &to).unwrap();
        to
    }

    #[test]
    fn a_moved_file_keeps_its_marks_keywords_copies_and_history() {
        let w = world("keep");
        let to = move_file(&w);
        let moved = w
            .cat
            .relink_moved(&w.root.join("b"), &[entry(&to)], |p| !p.exists())
            .unwrap();
        assert_eq!(moved, vec![(w.root.join("a/x.RAF"), to.clone())]);
        assert_eq!(
            w.cat.marks(&to, 0).unwrap().rating,
            4,
            "the rating followed"
        );
        assert!(
            w.cat.photo_id(&to, 1).unwrap().is_some(),
            "the copy followed"
        );
        assert_eq!(
            w.cat.load_history(&to, 0).unwrap().as_deref(),
            Some("history")
        );
        assert_eq!(
            w.cat
                .description_of(&to)
                .unwrap()
                .map(|d| d.title)
                .as_deref(),
            Some("Eiffel"),
            "the description followed"
        );
        assert_eq!(
            w.cat.capture_of(&to).unwrap().and_then(|c| c.iso),
            Some(3200),
            "the capture data followed"
        );
        let kw = w.cat.keywords().unwrap()[0].id;
        assert!(
            w.cat.file_has_keyword(kw, &to).unwrap(),
            "the keyword followed"
        );
        assert!(
            w.cat
                .photo_id(&w.root.join("a/x.RAF"), 0)
                .unwrap()
                .is_none(),
            "no row left behind"
        );
    }

    /// ⚠ The original still exists: the new file is a COPY, and must stay a separate photo.
    #[test]
    fn a_surviving_original_makes_it_a_copy_not_a_move() {
        let w = world("copy");
        let to = w.root.join("b/x.RAF");
        std::fs::copy(w.root.join("a/x.RAF"), &to).unwrap();
        // Same mtime as the original, as a Finder copy keeps it.
        let m = std::fs::metadata(w.root.join("a/x.RAF"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&to)
            .unwrap()
            .set_modified(m)
            .unwrap();
        let moved = w
            .cat
            .relink_moved(&w.root.join("b"), &[entry(&to)], |p| !p.exists())
            .unwrap();
        assert!(moved.is_empty());
        assert_eq!(
            w.cat.marks(&w.root.join("a/x.RAF"), 0).unwrap().rating,
            4,
            "untouched"
        );
    }

    /// ⚠ Two gone candidates with the same name, size and time: ambiguous, so neither is taken.
    #[test]
    fn two_candidates_are_ambiguous_and_nothing_is_relinked() {
        let w = world("ambiguous");
        std::fs::create_dir_all(w.root.join("c")).unwrap();
        let other = w.root.join("c/x.RAF");
        std::fs::copy(w.root.join("a/x.RAF"), &other).unwrap();
        let m = std::fs::metadata(w.root.join("a/x.RAF"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(m)
            .unwrap();
        w.cat
            .sync_folder(&w.root.join("c"), &[entry(&other)])
            .unwrap();
        std::fs::remove_file(&other).unwrap();
        let to = move_file(&w);
        let moved = w
            .cat
            .relink_moved(&w.root.join("b"), &[entry(&to)], |p| !p.exists())
            .unwrap();
        assert!(moved.is_empty(), "{moved:?}");
    }

    /// ⚠ A different size is a different photo, whatever its name.
    #[test]
    fn a_different_size_is_a_different_photo() {
        let w = world("size");
        std::fs::remove_file(w.root.join("a/x.RAF")).unwrap();
        let to = w.root.join("b/x.RAF");
        std::fs::write(&to, b"other bytes, longer than before").unwrap();
        let moved = w
            .cat
            .relink_moved(&w.root.join("b"), &[entry(&to)], |p| !p.exists())
            .unwrap();
        assert!(moved.is_empty());
    }

    /// ⚠⚠ A move Rustroom makes — into a folder the catalog has never seen, **renamed** on the way —
    /// takes every per-file row and keeps the photo in its collection (2026-10-01).
    #[test]
    fn move_photo_takes_every_row_to_a_new_folder_and_name() {
        let w = world("move_photo");
        let from = w.root.join("a/x.RAF");
        let c = w.cat.create_collection("Trips").unwrap();
        w.cat.add_to_collection(c, &from, 0).unwrap();
        let to = w.root.join("c/y.RAF");
        assert!(
            w.cat.knows(&from).unwrap() && !w.cat.knows(&to).unwrap(),
            "the sample"
        );

        w.cat.move_photo(&from, &to).unwrap();

        assert!(!w.cat.knows(&from).unwrap() && w.cat.knows(&to).unwrap());
        assert_eq!(w.cat.marks(&to, 0).unwrap().rating, 4, "the rating");
        assert!(w.cat.photo_id(&to, 1).unwrap().is_some(), "the copy");
        assert_eq!(
            w.cat.load_history(&to, 0).unwrap().as_deref(),
            Some("history")
        );
        assert_eq!(
            w.cat
                .description_of(&to)
                .unwrap()
                .map(|d| d.title)
                .as_deref(),
            Some("Eiffel")
        );
        assert_eq!(
            w.cat.capture_of(&to).unwrap().and_then(|c| c.iso),
            Some(3200)
        );
        let kw = w.cat.keywords().unwrap()[0].id;
        assert!(w.cat.file_has_keyword(kw, &to).unwrap(), "the keyword");
        assert_eq!(w.cat.collection_members(c).unwrap(), vec![(to.clone(), 0)]);
        assert!(
            w.cat
                .folders()
                .unwrap()
                .iter()
                .any(|f| f.path == w.root.join("c")),
            "the new folder is known"
        );
    }

    #[test]
    fn moving_a_photo_the_catalog_does_not_know_is_harmless() {
        let w = world("move_unknown");
        w.cat
            .move_photo(&w.root.join("z/none.RAF"), &w.root.join("b/none.RAF"))
            .unwrap();
        assert!(w.cat.knows(&w.root.join("a/x.RAF")).unwrap());
    }
}
