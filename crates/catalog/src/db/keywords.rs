//! Renaming, re-parenting and merging keywords (M9.8).
//!
//! ⚠ Its own file: `db.rs` is already the crate's largest. A child module, so it reaches the
//! private connection.
//!
//! ⚠⚠ **A merge is reversed EXACTLY, not by the deletion snapshot.** `restore` re-creates deleted
//! keywords under *new* ids, which is fine for undoing a delete but not here: an earlier undo entry
//! naming a merged-away keyword — its rename, say — must still name it after the merge is undone.
//! So [`KeywordMerge`] records rows with their original ids and `unmerge_keyword` re-inserts them.

use super::{split, Catalog, CatalogError};
use crate::snapshot::KeywordSnapshot;
use std::path::PathBuf;

/// What a rename or a move did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Done,
    /// A sibling already has this name; nothing changed. The caller may offer a merge into it.
    Collides(i64),
    /// The move would put a keyword under itself or one of its own descendants; nothing changed.
    Loop,
}

/// Everything one merge changed, so it can be reversed exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct KeywordMerge {
    pub from: i64,
    pub into: i64,
    /// Rows deleted — `from`, and any child merged away under it — deepest first.
    pub removed: Vec<KeywordSnapshot>,
    /// The assignments those rows carried: `(folder_id, file_name, keyword_id)`.
    pub removed_assignments: Vec<(i64, String, i64)>,
    /// Assignments added to a surviving keyword because a merged-away one had them and it did not.
    pub added: Vec<(i64, String, i64)>,
    /// Children re-parented under a survivor: `(id, old_parent)`.
    pub moved: Vec<(i64, i64)>,
}

impl Catalog {
    fn sibling_named(
        &self,
        name: &str,
        parent: Option<i64>,
        except: i64,
    ) -> Result<Option<i64>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT id FROM keywords
             WHERE name = ?1 AND COALESCE(parent_id, 0) = COALESCE(?2, 0) AND id != ?3",
        )?;
        let mut rows = st.query(rusqlite::params![name, parent, except])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    fn keyword_row(&self, id: i64) -> Result<KeywordSnapshot, CatalogError> {
        Ok(self.conn.query_row(
            "SELECT id, name, parent_id FROM keywords WHERE id = ?1",
            [id],
            |r| {
                Ok(KeywordSnapshot {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            },
        )?)
    }

    /// `id`'s parent, or `None` for a root.
    pub fn keyword_parent(&self, id: i64) -> Result<Option<i64>, CatalogError> {
        Ok(self.keyword_row(id)?.parent_id)
    }

    /// `id`'s name.
    pub fn keyword_name(&self, id: i64) -> Result<String, CatalogError> {
        Ok(self.keyword_row(id)?.name)
    }

    /// Rename `id`. A sibling of that name is a [`Change::Collides`], and nothing changes.
    pub fn rename_keyword(&self, id: i64, name: &str) -> Result<Change, CatalogError> {
        let row = self.keyword_row(id)?;
        if let Some(other) = self.sibling_named(name, row.parent_id, id)? {
            return Ok(Change::Collides(other));
        }
        self.conn.execute(
            "UPDATE keywords SET name = ?2 WHERE id = ?1",
            rusqlite::params![id, name],
        )?;
        Ok(Change::Done)
    }

    /// Move `id` under `parent` (`None`: to the top level).
    pub fn move_keyword(&self, id: i64, parent: Option<i64>) -> Result<Change, CatalogError> {
        if let Some(p) = parent {
            if p == id || self.keyword_subtree(id)?.contains(&p) {
                return Ok(Change::Loop);
            }
        }
        let row = self.keyword_row(id)?;
        if let Some(other) = self.sibling_named(&row.name, parent, id)? {
            return Ok(Change::Collides(other));
        }
        self.conn.execute(
            "UPDATE keywords SET parent_id = ?2 WHERE id = ?1",
            rusqlite::params![id, parent],
        )?;
        Ok(Change::Done)
    }

    /// `id` and every keyword beneath it.
    pub fn keyword_subtree(&self, id: i64) -> Result<Vec<i64>, CatalogError> {
        let mut st = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT id FROM sub",
        )?;
        let rows = st.query_map([id], |r| r.get(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every file carrying `id` or anything beneath it, once each — whose XMP a change to this
    /// part of the tree rewrites.
    pub fn keyword_files(&self, id: i64) -> Result<Vec<PathBuf>, CatalogError> {
        let mut st = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT DISTINCT f.path, pk.file_name
             FROM photo_keywords pk JOIN folders f ON f.id = pk.folder_id
             WHERE pk.keyword_id IN (SELECT id FROM sub)
             ORDER BY f.path, pk.file_name",
        )?;
        let rows = st.query_map([id], |r| {
            Ok(PathBuf::from(r.get::<_, String>(0)?).join(r.get::<_, String>(1)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Merge `from` into `into`: every file carrying `from` carries `into`, `from`'s children move
    /// under `into` (one whose name `into` already has merges into it, recursively), and `from` is
    /// deleted. One transaction; the record reverses it exactly.
    pub fn merge_keyword(&self, from: i64, into: i64) -> Result<KeywordMerge, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut rec = KeywordMerge {
            from,
            into,
            removed: Vec::new(),
            removed_assignments: Vec::new(),
            added: Vec::new(),
            moved: Vec::new(),
        };
        self.merge_into(from, into, &mut rec)?;
        tx.commit()?;
        Ok(rec)
    }

    fn merge_into(&self, from: i64, into: i64, rec: &mut KeywordMerge) -> Result<(), CatalogError> {
        let children: Vec<(i64, String)> = {
            let mut st = self
                .conn
                .prepare("SELECT id, name FROM keywords WHERE parent_id = ?1 ORDER BY id")?;
            let rows = st.query_map([from], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.filter_map(Result::ok).collect()
        };
        for (child, name) in children {
            match self.sibling_named(&name, Some(into), child)? {
                Some(twin) => self.merge_into(child, twin, rec)?,
                None => {
                    self.conn.execute(
                        "UPDATE keywords SET parent_id = ?2 WHERE id = ?1",
                        rusqlite::params![child, into],
                    )?;
                    rec.moved.push((child, from));
                }
            }
        }
        let own: Vec<(i64, String)> = {
            let mut st = self
                .conn
                .prepare("SELECT folder_id, file_name FROM photo_keywords WHERE keyword_id = ?1")?;
            let rows = st.query_map([from], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.filter_map(Result::ok).collect()
        };
        for (folder, file) in own {
            let n = self.conn.execute(
                "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![folder, file, into],
            )?;
            if n > 0 {
                rec.added.push((folder, file.clone(), into));
            }
            rec.removed_assignments.push((folder, file, from));
        }
        rec.removed.push(self.keyword_row(from)?);
        // ⚠ Childless by now — every child was moved or merged away above — so the cascade takes
        // only `from`'s own assignments, which the record already holds.
        self.conn
            .execute("DELETE FROM keywords WHERE id = ?1", [from])?;
        Ok(())
    }

    /// Reverse `rec` exactly: the deleted rows back with their original ids and assignments, the
    /// moved children back under their old parents, the added assignments gone.
    pub fn unmerge_keyword(&self, rec: &KeywordMerge) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        // `removed` is deepest-first, so reversed it is parents-first.
        for k in rec.removed.iter().rev() {
            tx.execute(
                "INSERT INTO keywords (id, name, parent_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![k.id, k.name, k.parent_id],
            )?;
        }
        for (folder, file, kw) in &rec.removed_assignments {
            tx.execute(
                "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![folder, file, kw],
            )?;
        }
        for (id, old) in &rec.moved {
            tx.execute(
                "UPDATE keywords SET parent_id = ?2 WHERE id = ?1",
                rusqlite::params![id, old],
            )?;
        }
        for (folder, file, kw) in &rec.added {
            tx.execute(
                "DELETE FROM photo_keywords WHERE folder_id = ?1 AND file_name = ?2 AND keyword_id = ?3",
                rusqlite::params![folder, file, kw],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The distinct files named by `(folder_id, file_name, _)` assignments — a deletion
    /// snapshot's, whose XMP its undo and redo rewrite. A folder no longer in the catalog is
    /// skipped.
    pub fn assignment_paths(
        &self,
        assignments: &[(i64, String, i64)],
    ) -> Result<Vec<PathBuf>, CatalogError> {
        let mut out: Vec<PathBuf> = Vec::new();
        for (folder, file, _) in assignments {
            let path: Option<String> = self
                .conn
                .query_row("SELECT path FROM folders WHERE id = ?1", [folder], |r| {
                    r.get(0)
                })
                .ok();
            if let Some(p) = path {
                let f = PathBuf::from(p).join(file);
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        Ok(out)
    }

    /// Whether the file `photo` carries keyword `id` directly — for tests and the app's checks.
    pub fn file_has_keyword(&self, id: i64, photo: &std::path::Path) -> Result<bool, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(false);
        };
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM photo_keywords pk JOIN folders f ON f.id = pk.folder_id
                            WHERE f.path = ?1 AND pk.file_name = ?2 AND pk.keyword_id = ?3)",
            rusqlite::params![folder, name, id],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::Change;
    use crate::db::Catalog;
    use crate::CatalogEntry;
    use std::path::{Path, PathBuf};

    fn cat() -> Catalog {
        let c = Catalog::open_in_memory().unwrap();
        let entries: Vec<CatalogEntry> = ["a.raf", "b.raf", "c.raf"]
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from("/p").join(n),
                file_name: n.to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        c.sync_folder(Path::new("/p"), &entries).unwrap();
        c
    }

    fn p(n: &str) -> PathBuf {
        PathBuf::from("/p").join(n)
    }

    /// The whole keyword state, ids included — what "exactly as before" is measured against.
    /// Keyword rows `(id, name, parent)` and assignments `(folder, file, keyword)`.
    type State = (Vec<(i64, String, Option<i64>)>, Vec<(i64, String, i64)>);

    fn state(c: &Catalog) -> State {
        let mut st = c
            .conn
            .prepare("SELECT id, name, parent_id FROM keywords ORDER BY id")
            .unwrap();
        let k = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let mut st = c
            .conn
            .prepare("SELECT folder_id, file_name, keyword_id FROM photo_keywords ORDER BY 1, 2, 3")
            .unwrap();
        let a = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        (k, a)
    }

    #[test]
    fn a_rename_renames_and_a_sibling_collision_changes_nothing() {
        let c = cat();
        let paris = c.create_keyword("Pariss", None).unwrap();
        let other = c.create_keyword("Paris", None).unwrap();
        assert_eq!(
            c.rename_keyword(paris, "Paris").unwrap(),
            Change::Collides(other)
        );
        assert_eq!(c.keyword_name(paris).unwrap(), "Pariss");
        assert_eq!(c.rename_keyword(paris, "Lyon").unwrap(), Change::Done);
        assert_eq!(c.keyword_name(paris).unwrap(), "Lyon");
        // ⚠ The same name under ANOTHER parent is no collision.
        let fr = c.create_keyword("France", None).unwrap();
        let nested = c.create_keyword("Nice", Some(fr)).unwrap();
        assert_eq!(c.rename_keyword(nested, "Paris").unwrap(), Change::Done);
    }

    #[test]
    fn a_move_reparents_refuses_a_loop_and_reports_a_collision() {
        let c = cat();
        let eu = c.create_keyword("Europe", None).unwrap();
        let fr = c.create_keyword("France", Some(eu)).unwrap();
        let paris = c.create_keyword("Paris", Some(fr)).unwrap();
        assert_eq!(
            c.move_keyword(eu, Some(paris)).unwrap(),
            Change::Loop,
            "under a descendant"
        );
        assert_eq!(
            c.move_keyword(fr, Some(fr)).unwrap(),
            Change::Loop,
            "under itself"
        );
        assert_eq!(c.move_keyword(paris, None).unwrap(), Change::Done);
        assert_eq!(c.keyword_parent(paris).unwrap(), None);
        let twin = c.create_keyword("Paris", Some(fr)).unwrap();
        assert_eq!(
            c.move_keyword(paris, Some(fr)).unwrap(),
            Change::Collides(twin)
        );
        assert_eq!(
            c.keyword_parent(paris).unwrap(),
            None,
            "a collision moves nothing"
        );
    }

    /// ⚠⚠ The merge the author chose, with a RECURSIVE collision: both "France" keywords have a
    /// "Paris", which must merge too, while "Lyon" simply moves across. Then unmerge must put back
    /// the exact prior state, ids included.
    #[test]
    fn a_merge_combines_recursively_and_unmerges_exactly() {
        let c = cat();
        let fr = c.create_keyword("France", None).unwrap();
        let fr2 = c.create_keyword("Frankreich", None).unwrap();
        let paris = c.create_keyword("Paris", Some(fr)).unwrap();
        let paris2 = c.create_keyword("Paris", Some(fr2)).unwrap();
        let lyon2 = c.create_keyword("Lyon", Some(fr2)).unwrap();
        c.assign_keyword(fr2, &p("a.raf")).unwrap();
        c.assign_keyword(paris2, &p("b.raf")).unwrap();
        c.assign_keyword(paris, &p("b.raf")).unwrap(); // already on the survivor: not "added"
        c.assign_keyword(lyon2, &p("c.raf")).unwrap();
        let before = state(&c);

        let rec = c.merge_keyword(fr2, fr).unwrap();
        assert!(c.file_has_keyword(fr, &p("a.raf")).unwrap());
        assert!(c.file_has_keyword(paris, &p("b.raf")).unwrap());
        assert_eq!(
            c.keyword_parent(lyon2).unwrap(),
            Some(fr),
            "Lyon moved across"
        );
        assert!(
            c.file_has_keyword(lyon2, &p("c.raf")).unwrap(),
            "and kept its photo"
        );
        let ids: Vec<i64> = state(&c).0.iter().map(|k| k.0).collect();
        assert!(
            !ids.contains(&fr2) && !ids.contains(&paris2),
            "both merged away"
        );
        assert_eq!(
            rec.added.len(),
            1,
            "b already had Paris; only a gained France"
        );

        c.unmerge_keyword(&rec).unwrap();
        assert_eq!(
            state(&c),
            before,
            "unmerge must restore the exact state, ids included"
        );
    }

    #[test]
    fn a_keywords_files_include_its_subtree_once_each() {
        let c = cat();
        let fr = c.create_keyword("France", None).unwrap();
        let paris = c.create_keyword("Paris", Some(fr)).unwrap();
        c.assign_keyword(fr, &p("a.raf")).unwrap();
        c.assign_keyword(paris, &p("a.raf")).unwrap();
        c.assign_keyword(paris, &p("b.raf")).unwrap();
        assert_eq!(c.keyword_files(fr).unwrap(), vec![p("a.raf"), p("b.raf")]);
        assert_eq!(
            c.keyword_files(paris).unwrap(),
            vec![p("a.raf"), p("b.raf")]
        );
    }
}
