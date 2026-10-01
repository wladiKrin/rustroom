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
use crate::snapshot::{CatalogSnapshot, KeywordSnapshot};
use std::path::{Path, PathBuf};

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

/// A keyword in the tree, flattened for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordRow {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    /// How deep in the tree, for indenting. Roots are 0.
    pub depth: usize,
    /// How many *files* carry this keyword directly — **not** counting children.
    pub count: i64,
}

/// The photos taken on the selection's capture day(s), and the keywords they carry.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DayCounts {
    /// How many photos those days hold, the selection excluded.
    pub neighbours: usize,
    /// How many of them carry each keyword.
    pub counts: std::collections::HashMap<i64, usize>,
}

impl Catalog {
    /// Create a keyword under `parent`, or return the existing one of that name and parent.
    ///
    /// ⚠ A duplicate is not an error, for the reason `create_collection` gives: the name is the
    /// identity a user thinks in.
    pub fn create_keyword(&self, name: &str, parent: Option<i64>) -> Result<i64, CatalogError> {
        self.create_keyword_reporting(name, parent)
            .map(|(id, _)| id)
    }

    /// [`Self::create_keyword`], and whether this call made the row (M10.8a): an undo of a
    /// "create" that returned an existing keyword must not delete it.
    pub fn create_keyword_reporting(
        &self,
        name: &str,
        parent: Option<i64>,
    ) -> Result<(i64, bool), CatalogError> {
        let created = self.conn.execute(
            "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
            rusqlite::params![name, parent],
        )? > 0;
        let id = self.conn.query_row(
            "SELECT id FROM keywords WHERE name = ?1 AND COALESCE(parent_id, 0) = COALESCE(?2, 0)",
            rusqlite::params![name, parent],
            |r| r.get(0),
        )?;
        Ok((id, created))
    }

    /// Put back a keyword an undo removed, under its **original id** — `unmerge_keyword`'s rule, so
    /// an undo step recorded before it still names it.
    pub fn recreate_keyword(
        &self,
        id: i64,
        name: &str,
        parent: Option<i64>,
    ) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO keywords (id, name, parent_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, parent],
        )?;
        Ok(())
    }

    /// The whole tree, depth-first and name-ordered within each level.
    ///
    /// ⚠ Ordered in SQL so the tree does not reshuffle between frames.
    pub fn keywords(&self) -> Result<Vec<KeywordRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id, name, parent_id, depth, path) AS (
                 SELECT id, name, parent_id, 0, lower(name)
                 FROM keywords WHERE parent_id IS NULL
                 UNION ALL
                 SELECT k.id, k.name, k.parent_id, t.depth + 1, t.path || '/' || lower(k.name)
                 FROM keywords k JOIN tree t ON k.parent_id = t.id
             )
             SELECT t.id, t.name, t.parent_id, t.depth,
                    (SELECT count(*) FROM photo_keywords pk WHERE pk.keyword_id = t.id)
             FROM tree t ORDER BY t.path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(KeywordRow {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                depth: r.get::<_, i64>(3)? as usize,
                count: r.get(4)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// How much a keyword deletion would take with it: `(descendants, assignments)`.
    ///
    /// ⚠ Descendants **excludes** the keyword itself, and assignments **includes** the whole
    /// subtree's — the two numbers the confirmation says out loud.
    ///
    /// ⚠ The same recursive CTE `keyword_members` uses, so nothing new walks the subtree.
    pub fn keyword_delete_scope(&self, id: i64) -> Result<(usize, usize), CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT
               (SELECT count(*) FROM sub) - 1,
               (SELECT count(*) FROM photo_keywords WHERE keyword_id IN (SELECT id FROM sub))",
        )?;
        let (d, a): (i64, i64) = stmt.query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((d.max(0) as usize, a.max(0) as usize))
    }

    /// Remove a keyword. ⚠ Its **children go with it**, and every assignment of any of them.
    pub fn delete_keyword(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        // ⚠ The same recursive CTE `keyword_members` uses, and for the same reason: the cascade
        // takes the whole subtree, so the snapshot must hold the whole subtree. The `depth`
        // ordering is what puts **parents before children**, which is what lets the restore remap
        // `parent_id` to an id that already exists.
        let mut st = tx.prepare(
            "WITH RECURSIVE sub(id, name, parent_id, depth) AS (
                 SELECT id, name, parent_id, 0 FROM keywords WHERE id = ?1
                 UNION ALL
                 SELECT k.id, k.name, k.parent_id, sub.depth + 1
                 FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT id, name, parent_id FROM sub ORDER BY depth, id",
        )?;
        let tree: Vec<KeywordSnapshot> = st
            .query_map([id], |r| {
                Ok(KeywordSnapshot {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut assignments = Vec::new();
        for k in &tree {
            let mut st = tx.prepare(
                "SELECT folder_id, file_name, keyword_id FROM photo_keywords WHERE keyword_id = ?1",
            )?;
            let rows = st.query_map([k.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            assignments.extend(rows.filter_map(Result::ok));
        }

        tx.execute("DELETE FROM keywords WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Keyword { tree, assignments })
    }

    /// Tag a **file** — every copy of it, now and in future.
    ///
    /// ⚠ Keyed on `(folder, file_name)` rather than on a photo row, which is what makes that true
    /// with no inheritance rule to maintain.
    /// Tag `photo` with `keyword`; `true` when it was not already tagged (M10.8a — what lets an
    /// undo remove exactly the tags this call added).
    pub fn assign_keyword(&self, keyword: i64, photo: &Path) -> Result<bool, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(false);
        };
        Ok(self.conn.execute(
            "INSERT INTO photo_keywords (folder_id, file_name, keyword_id)
             SELECT f.id, ?2, ?3 FROM folders f WHERE f.path = ?1
             ON CONFLICT DO NOTHING",
            rusqlite::params![folder, name, keyword],
        )? > 0)
    }

    /// Untag `photo`; `true` when it was tagged.
    pub fn unassign_keyword(&self, keyword: i64, photo: &Path) -> Result<bool, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(false);
        };
        Ok(self.conn.execute(
            "DELETE FROM photo_keywords
             WHERE keyword_id = ?3 AND file_name = ?2
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![folder, name, keyword],
        )? > 0)
    }

    /// The photos taken on the same capture day(s) as `photos` — `photos` themselves excluded — and
    /// how many of them carry each keyword (M10.8a's same-day evidence).
    ///
    /// ⚠ A day is `date(taken, 'unixepoch')`, the Dates tree's day: `taken` is wall-clock time
    /// stored as if UTC, so this is the date on the camera. A photo with no time has no day.
    pub fn day_keyword_counts(&self, photos: &[PathBuf]) -> Result<DayCounts, CatalogError> {
        let mut own = std::collections::HashSet::new();
        let mut days = std::collections::BTreeSet::new();
        let mut day_of = self.conn.prepare(
            "SELECT pc.folder_id, date(pc.taken, 'unixepoch') FROM photo_capture pc
             JOIN folders f ON f.id = pc.folder_id
             WHERE f.path = ?1 AND pc.file_name = ?2",
        )?;
        for photo in photos {
            let Some((folder, name)) = split(photo) else {
                continue;
            };
            let rows = day_of.query_map(rusqlite::params![folder, name], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for (fid, day) in rows.filter_map(Result::ok) {
                own.insert((fid, name.clone()));
                days.extend(day);
            }
        }
        let mut on_day = self.conn.prepare(
            "SELECT pc.folder_id, pc.file_name, pk.keyword_id FROM photo_capture pc
             LEFT JOIN photo_keywords pk
                    ON pk.folder_id = pc.folder_id AND pk.file_name = pc.file_name
             WHERE date(pc.taken, 'unixepoch') = ?1",
        )?;
        let mut neighbours = std::collections::HashSet::new();
        let mut counts = std::collections::HashMap::new();
        for day in &days {
            let rows = on_day.query_map([day], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for (fid, file, kw) in rows.filter_map(Result::ok) {
                if own.contains(&(fid, file.clone())) {
                    continue;
                }
                neighbours.insert((fid, file));
                if let Some(k) = kw {
                    *counts.entry(k).or_insert(0) += 1;
                }
            }
        }
        Ok(DayCounts {
            neighbours: neighbours.len(),
            counts,
        })
    }

    /// For each `j` in `keywords`, how many files carry both `j` and each other keyword `k`, keyed
    /// `(j, k)` — M10.8a's companion evidence.
    pub fn companion_counts(
        &self,
        keywords: &[i64],
    ) -> Result<std::collections::HashMap<(i64, i64), usize>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT b.keyword_id, count(*) FROM photo_keywords a
             JOIN photo_keywords b ON b.folder_id = a.folder_id AND b.file_name = a.file_name
             WHERE a.keyword_id = ?1 AND b.keyword_id != ?1
             GROUP BY b.keyword_id",
        )?;
        let mut out = std::collections::HashMap::new();
        for &j in keywords {
            let rows = stmt.query_map([j], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for (k, n) in rows.filter_map(Result::ok) {
                out.insert((j, k), n as usize);
            }
        }
        Ok(out)
    }

    /// How many of `files` carry each keyword — the tri-state a photo menu needs.
    ///
    /// ⚠⚠ **Direct assignments only, never the subtree.** `keyword_members` expands descendants
    /// because a *search* for `Europe` must find `Paris`; a menu toggle removes an
    /// **assignment**, so a photo tagged only `Paris` must show `Europe` unchecked. Ticking it
    /// from the subtree would make unticking it appear to do nothing.
    pub fn keyword_counts(
        &self,
        files: &[PathBuf],
    ) -> Result<std::collections::HashMap<i64, usize>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT pk.keyword_id FROM photo_keywords pk
             JOIN folders f ON f.id = pk.folder_id
             WHERE f.path = ?1 AND pk.file_name = ?2",
        )?;
        let mut counts = std::collections::HashMap::new();
        for file in files {
            let Some((folder, name)) = split(file) else {
                continue;
            };
            let rows = stmt.query_map(rusqlite::params![folder, name], |r| r.get::<_, i64>(0))?;
            for id in rows.filter_map(Result::ok) {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    /// Every photo carrying `keyword` **or any of its descendants**, as `(path, copy)`.
    ///
    /// ⚠⚠ The subtree is resolved by a recursive CTE at query time, never stored. Storing the
    /// implication would mean rewriting every affected row whenever a keyword is re-parented —
    /// two facts that can disagree, which this project has been bitten by twice.
    ///
    /// ⚠ Joins back to `photos` on `(folder, file_name)`, so **every copy of a tagged file**
    /// comes back. That is the point of keying on the file.
    pub fn keyword_members(&self, keyword: i64) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT DISTINCT f.path, p.file_name, p.copy
             FROM photo_keywords pk
             JOIN sub       ON sub.id = pk.keyword_id
             JOIN folders f ON f.id = pk.folder_id
             JOIN photos  p ON p.folder_id = pk.folder_id AND p.file_name = pk.file_name
             ORDER BY lower(f.path), lower(p.file_name), p.copy",
        )?;
        let rows = stmt.query_map([keyword], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            let copy: u32 = r.get(2)?;
            Ok((PathBuf::from(folder).join(name), copy))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

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
    use super::*;
    use crate::db::test_support::{entries_in, entry};
    use crate::CatalogEntry;

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

    #[test]
    fn two_root_keywords_of_the_same_name_are_refused() {
        // ⚠⚠ **The `COALESCE` index is the only thing preventing this.** A plain
        // `UNIQUE(parent_id, name)` lets both through, because SQLite treats NULLs as *distinct*
        // in a unique constraint — you would get two identical-looking roots in the tree with no
        // way to tell them apart.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_keyword("Paris", None).unwrap();
        let b = cat.create_keyword("Paris", None).unwrap();
        assert_eq!(a, b, "the second must return the existing one");
        assert_eq!(cat.keywords().unwrap().len(), 1);
    }

    #[test]
    fn the_same_name_under_different_parents_is_allowed() {
        // ⚠ The discriminating other half. "Paris" under both France and Texas is legitimate, and
        // an over-broad unique constraint would refuse it.
        let cat = Catalog::open_in_memory().unwrap();
        let fr = cat.create_keyword("France", None).unwrap();
        let tx = cat.create_keyword("Texas", None).unwrap();
        let a = cat.create_keyword("Paris", Some(fr)).unwrap();
        let b = cat.create_keyword("Paris", Some(tx)).unwrap();
        assert_ne!(a, b);
        assert_eq!(cat.keywords().unwrap().len(), 4);
    }

    #[test]
    fn a_keyword_applies_to_every_copy_of_its_file() {
        // ⚠⚠ The decision the schema shape exists for. A keyword describes what is *in* the
        // picture, which is identical across copies — so it is keyed on the file, and no
        // inheritance rule is needed because there is only one fact.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();

        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        let mut members = cat.keyword_members(k).unwrap();
        members.sort();
        assert_eq!(
            members,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/x/a.RAF"), 1)
            ],
            "both copies must match a keyword assigned once"
        );
    }

    #[test]
    fn a_copy_created_after_tagging_still_matches() {
        // ⚠ The exact case that would need an "inherit on create" rule under per-copy storage.
        // Keyed on the file, it needs nothing at all.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();

        assert_eq!(cat.keyword_members(k).unwrap().len(), 2);
    }

    #[test]
    fn a_parent_keyword_finds_its_childrens_photos() {
        // ⚠⚠ The payoff, and the reason hierarchy was chosen over flat: searching Europe finds
        // photos tagged Paris. Derived by a recursive query, never stored.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let france = cat.create_keyword("France", Some(europe)).unwrap();
        let paris = cat.create_keyword("Paris", Some(france)).unwrap();

        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(europe, Path::new("/x/b.RAF")).unwrap();

        let mut from_parent = cat.keyword_members(europe).unwrap();
        from_parent.sort();
        assert_eq!(
            from_parent,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/x/b.RAF"), 0)
            ],
            "Europe must find the photo tagged only Paris"
        );
        assert_eq!(
            cat.keyword_members(paris).unwrap().len(),
            1,
            "but Paris must not find Europe's"
        );
    }

    #[test]
    fn deleting_a_parent_removes_its_children_and_their_assignments() {
        // ⚠ Destructive and deliberate. Re-parenting orphans to the root would silently flatten a
        // taxonomy you built, which is worse than an obvious deletion.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let paris = cat.create_keyword("Paris", Some(europe)).unwrap();
        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();

        cat.delete_keyword(europe).unwrap();

        assert!(cat.keywords().unwrap().is_empty(), "both are gone");
        assert_eq!(cat.photo_count().unwrap(), 1, "the photo is untouched");
    }

    #[test]
    fn a_renamed_file_keeps_its_keywords() {
        // ⚠⚠ `photo_keywords` is keyed by **file name**, so `rename_photo` must update it in the
        // same operation — the third instance of that integration.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(
            cat.keyword_members(k).unwrap(),
            vec![(PathBuf::from("/x/b.RAF"), 0)]
        );
    }

    #[test]
    fn unassigning_leaves_the_keyword_and_the_photo() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.unassign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        assert!(cat.keyword_members(k).unwrap().is_empty());
        assert_eq!(cat.keywords().unwrap().len(), 1, "the keyword survives");
        assert_eq!(cat.photo_count().unwrap(), 1, "and so does the photo");
    }

    #[test]
    fn keyword_counts_count_direct_assignments_and_not_the_subtree() {
        // ⚠⚠ The trap this exists for: `keyword_members` deliberately expands descendants, so a
        // search for Europe finds a photo tagged only Paris. A menu **toggle** must not — it
        // removes an assignment, and a ticked Europe that unticking cannot clear reads as the
        // control being broken.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let paris = cat.create_keyword("Paris", Some(europe)).unwrap();
        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(europe, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(paris, Path::new("/x/b.RAF")).unwrap();

        let files = vec![PathBuf::from("/x/a.RAF"), PathBuf::from("/x/b.RAF")];
        let counts = cat.keyword_counts(&files).unwrap();

        assert_eq!(counts.get(&paris), Some(&2), "both carry Paris directly");
        assert_eq!(
            counts.get(&europe),
            Some(&1),
            "only one carries Europe itself — b.RAF is tagged Paris, which is not the same thing"
        );
    }

    #[test]
    fn an_untagged_file_contributes_no_counts() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.create_keyword("Paris", None).unwrap();

        let counts = cat.keyword_counts(&[PathBuf::from("/x/a.RAF")]).unwrap();

        assert!(counts.is_empty(), "an absent keyword is absent, not zero");
    }

    #[test]
    fn keywords_come_back_depth_first_in_name_order() {
        // ⚠ Ordered in SQL, so the tree does not reshuffle between frames — the sparse-index
        // class of bug this project has hit three times in the Library.
        let cat = Catalog::open_in_memory().unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        cat.create_keyword("Spain", Some(europe)).unwrap();
        cat.create_keyword("France", Some(europe)).unwrap();
        cat.create_keyword("Asia", None).unwrap();

        let got: Vec<(String, usize)> = cat
            .keywords()
            .unwrap()
            .into_iter()
            .map(|k| (k.name, k.depth))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Asia".to_string(), 0),
                ("Europe".to_string(), 0),
                ("France".to_string(), 1),
                ("Spain".to_string(), 1),
            ]
        );
    }

    #[test]
    fn the_delete_scope_counts_the_subtree_but_not_the_keyword_itself() {
        let c = Catalog::open_in_memory().unwrap();
        let europe = c.create_keyword("Europe", None).unwrap();
        let france = c.create_keyword("France", Some(europe)).unwrap();
        c.create_keyword("Paris", Some(france)).unwrap();
        assert_eq!(c.keyword_delete_scope(europe).unwrap(), (2, 0));
        assert_eq!(c.keyword_delete_scope(france).unwrap().0, 1);
    }

    /// ⚠ Both halves. A dialog that never fires and one that always fires are equally wrong, and
    /// only the pair of assertions tells them apart.
    #[test]
    fn a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not() {
        let c = Catalog::open_in_memory().unwrap();
        let leaf = c.create_keyword("Paris", None).unwrap();
        assert_eq!(c.keyword_delete_scope(leaf).unwrap(), (0, 0), "no dialog");

        let parent = c.create_keyword("Europe", None).unwrap();
        c.create_keyword("France", Some(parent)).unwrap();
        assert_ne!(c.keyword_delete_scope(parent).unwrap(), (0, 0), "dialog");
    }

    /// ⚠ An assignment alone is enough to confirm, with no descendants at all — the second half
    /// of the rule, which the descendant tests cannot see.
    #[test]
    fn an_assignment_alone_is_enough_to_need_a_confirmation() {
        let c = Catalog::open_in_memory().unwrap();
        c.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let k = c.create_keyword("Paris", None).unwrap();
        c.assign_keyword(k, Path::new("/a/one.RAF")).unwrap();
        assert_eq!(c.keyword_delete_scope(k).unwrap(), (0, 1));
    }

    /// M10.8a: a write reports whether it changed anything — what makes a tag's undo exact.
    #[test]
    fn tagging_reports_whether_it_changed_anything() {
        let c = cat();
        let k = c.create_keyword("Berlin", None).unwrap();
        assert!(c.assign_keyword(k, &p("a.raf")).unwrap());
        assert!(
            !c.assign_keyword(k, &p("a.raf")).unwrap(),
            "a second tag changed something"
        );
        assert!(c.unassign_keyword(k, &p("a.raf")).unwrap());
        assert!(
            !c.unassign_keyword(k, &p("a.raf")).unwrap(),
            "removing an absent tag changed something"
        );
    }

    /// ⚠ `create_keyword` returns an existing keyword's id for a name already there, so an undo of
    /// "create" must know whether this call made the row — or it deletes a keyword it never created.
    #[test]
    fn creating_reports_a_new_keyword_and_recreating_keeps_its_id() {
        let c = cat();
        let (id, new) = c.create_keyword_reporting("Berlin", None).unwrap();
        assert!(new);
        assert_eq!(
            c.create_keyword_reporting("Berlin", None).unwrap(),
            (id, false)
        );
        c.delete_keyword(id).unwrap();
        c.recreate_keyword(id, "Berlin", None).unwrap();
        assert!(c
            .keywords()
            .unwrap()
            .iter()
            .any(|k| k.id == id && k.name == "Berlin"));
    }

    /// The selection's capture day(s): its other photos, and which keywords they carry. Neither
    /// the next day, nor a photo without a date, nor the selection's own tags are evidence.
    #[test]
    fn day_counts_cover_the_selections_days_and_nothing_else() {
        let c = Catalog::open_in_memory().unwrap();
        let names = [
            "sel.raf",
            "same1.raf",
            "same2.raf",
            "other.raf",
            "undated.raf",
        ];
        let entries: Vec<CatalogEntry> = names
            .iter()
            .map(|n| CatalogEntry {
                path: p(n),
                file_name: n.to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        c.sync_folder(Path::new("/p"), &entries).unwrap();
        let midnight = 1_700_000_000 - 1_700_000_000 % 86_400;
        let at = |n: &str, taken: Option<i64>| {
            let cap = core_image::Capture {
                taken,
                ..Default::default()
            };
            c.set_capture(&p(n), &cap).unwrap();
        };
        at("sel.raf", Some(midnight + 3_600));
        at("same1.raf", Some(midnight + 7_200));
        at("same2.raf", Some(midnight + 80_000));
        at("other.raf", Some(midnight + 86_400 + 10));
        at("undated.raf", None);
        let street = c.create_keyword("Street", None).unwrap();
        let night = c.create_keyword("Night", None).unwrap();
        c.assign_keyword(street, &p("same1.raf")).unwrap();
        c.assign_keyword(street, &p("same2.raf")).unwrap();
        c.assign_keyword(night, &p("other.raf")).unwrap();
        c.assign_keyword(night, &p("sel.raf")).unwrap();
        let d = c.day_keyword_counts(&[p("sel.raf")]).unwrap();
        assert_eq!(
            d.neighbours, 2,
            "the photos taken that day, the selection excluded"
        );
        assert_eq!(d.counts.get(&street), Some(&2));
        assert_eq!(
            d.counts.get(&night),
            None,
            "the next day's photo, or the selection's own tag, counted"
        );
    }

    #[test]
    fn companion_counts_count_the_files_carrying_both() {
        let c = cat();
        let berlin = c.create_keyword("Berlin", None).unwrap();
        let street = c.create_keyword("Street", None).unwrap();
        let night = c.create_keyword("Night", None).unwrap();
        for f in ["a.raf", "b.raf"] {
            c.assign_keyword(berlin, &p(f)).unwrap();
            c.assign_keyword(street, &p(f)).unwrap();
        }
        c.assign_keyword(berlin, &p("c.raf")).unwrap();
        c.assign_keyword(night, &p("c.raf")).unwrap();
        let m = c.companion_counts(&[berlin]).unwrap();
        assert_eq!(m.get(&(berlin, street)), Some(&2));
        assert_eq!(m.get(&(berlin, night)), Some(&1));
        assert_eq!(m.get(&(berlin, berlin)), None);
    }

    /// The suggestion cache's key: every write moves it, a read does not.
    #[test]
    fn the_change_count_moves_on_a_write_and_not_on_a_read() {
        let c = cat();
        let before = c.change_count();
        let _ = c.keywords().unwrap();
        assert_eq!(c.change_count(), before);
        c.create_keyword("Berlin", None).unwrap();
        assert!(c.change_count() > before);
    }
}
