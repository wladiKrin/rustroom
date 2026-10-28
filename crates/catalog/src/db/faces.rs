//! Faces and the people they belong to (M10.18c). ⚠⚠ The catalog is their only home — never a file, a sidecar or
//! Immich (the author's call). Immich's people and faces are copies of the server's, replaced on each refresh;
//! local faces are found here and named by the user.
//!
//! ⚠ A face the user named or confirmed is never touched by a new search ([`Catalog::set_local_faces`] replaces
//! only the unnamed, unconfirmed ones), and "not a face" is a flag, so its undo is a flip.

use super::{split, Catalog, CatalogError};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A face a search found, before it has an id.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFace {
    /// `[x0, y0, x1, y1]`, normalised over the upright photo.
    pub bbox: [f32; 4],
    /// SFace's 128 numbers, of unit length.
    pub embedding: Vec<f32>,
}

/// One stored face.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceRow {
    pub id: i64,
    pub photo: PathBuf,
    pub bbox: [f32; 4],
    /// `None` for an Immich face — the server exposes none.
    pub embedding: Option<Vec<f32>>,
    pub person: Option<i64>,
    pub confirmed: bool,
    pub immich: bool,
}

/// One person, and how many photos they are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonRow {
    pub id: i64,
    pub name: String,
    pub immich_id: Option<String>,
    pub photos: i64,
}

/// What a face's naming is, for an undo: `(person, confirmed, dismissed)`.
pub type FaceState = (Option<i64>, bool, bool);

fn blob(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn unblob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

const FACE_COLS: &str =
    "fa.id, f.path, fa.file_name, fa.x0, fa.y0, fa.x1, fa.y1, fa.embedding, fa.person_id, \
                         fa.confirmed, fa.source";

fn face_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FaceRow> {
    let emb: Option<Vec<u8>> = r.get(7)?;
    Ok(FaceRow {
        id: r.get(0)?,
        photo: PathBuf::from(r.get::<_, String>(1)?).join(r.get::<_, String>(2)?),
        bbox: [
            r.get::<_, f64>(3)? as f32,
            r.get::<_, f64>(4)? as f32,
            r.get::<_, f64>(5)? as f32,
            r.get::<_, f64>(6)? as f32,
        ],
        embedding: emb.map(|b| unblob(&b)),
        person: r.get(8)?,
        confirmed: r.get::<_, i64>(9)? != 0,
        immich: r.get::<_, String>(10)? == "immich",
    })
}

impl Catalog {
    /// A search's answer for `photo`: its unnamed, unconfirmed local faces replaced by `faces`, and the photo
    /// marked read. ⚠ Named and confirmed faces stay — a new search never moves a name.
    pub fn set_local_faces(&self, photo: &Path, faces: &[NewFace]) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        let tx = self.conn.unchecked_transaction()?;
        let fid: Option<i64> = tx
            .query_row("SELECT id FROM folders WHERE path = ?1", [&folder], |r| {
                r.get(0)
            })
            .ok();
        let Some(fid) = fid else {
            return Ok(());
        };
        tx.execute(
            "DELETE FROM faces WHERE folder_id = ?1 AND file_name = ?2 AND source = 'local'
             AND person_id IS NULL AND confirmed = 0 AND dismissed = 0",
            rusqlite::params![fid, name],
        )?;
        for f in faces {
            let b = f.bbox;
            tx.execute(
                "INSERT INTO faces (folder_id, file_name, x0, y0, x1, y1, embedding, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'local')",
                rusqlite::params![fid, name, b[0], b[1], b[2], b[3], blob(&f.embedding)],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO faces_read (folder_id, file_name) VALUES (?1, ?2)",
            rusqlite::params![fid, name],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Those of `photos` never searched for faces.
    pub fn files_without_faces_read(
        &self,
        photos: &[PathBuf],
    ) -> Result<Vec<PathBuf>, CatalogError> {
        let mut st = self.conn.prepare_cached(
            "SELECT 1 FROM faces_read WHERE folder_id = (SELECT id FROM folders WHERE path = ?1) AND file_name = ?2",
        )?;
        let mut out = Vec::new();
        for p in photos {
            let Some((folder, name)) = split(p) else {
                continue;
            };
            if !st.exists(rusqlite::params![folder, name])? {
                out.push(p.clone());
            }
        }
        Ok(out)
    }

    /// Every face of `photo`, dismissed ones left out.
    pub fn faces_of(&self, photo: &Path) -> Result<Vec<FaceRow>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare(&format!(
            "SELECT {FACE_COLS} FROM faces fa JOIN folders f ON f.id = fa.folder_id
             WHERE f.path = ?1 AND fa.file_name = ?2 AND fa.dismissed = 0 ORDER BY fa.x0"
        ))?;
        let rows = st.query_map(rusqlite::params![folder, name], face_row)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every local face nobody has named yet — the People view's "Who is this?".
    pub fn unnamed_local_faces(&self) -> Result<Vec<FaceRow>, CatalogError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {FACE_COLS} FROM faces fa JOIN folders f ON f.id = fa.folder_id
             WHERE fa.source = 'local' AND fa.person_id IS NULL AND fa.dismissed = 0"
        ))?;
        let rows = st.query_map([], face_row)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every named local face with its person — what a group is compared against.
    pub fn named_local_faces(&self) -> Result<Vec<FaceRow>, CatalogError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {FACE_COLS} FROM faces fa JOIN folders f ON f.id = fa.folder_id
             WHERE fa.source = 'local' AND fa.person_id IS NOT NULL AND fa.dismissed = 0"
        ))?;
        let rows = st.query_map([], face_row)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every "No" ever answered: `(face, person)`.
    pub fn face_rejections(&self) -> Result<HashSet<(i64, i64)>, CatalogError> {
        let mut st = self
            .conn
            .prepare("SELECT face_id, person_id FROM face_rejections")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Everyone, by name, with the photos the catalog has of them: every Immich person (who may have none here yet —
    /// their own photos are the server's), and each local person with at least one.
    pub fn people(&self) -> Result<Vec<PersonRow>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT p.id, p.name, p.immich_person_id, COUNT(DISTINCT fa.folder_id || '/' || fa.file_name) AS n
             FROM people p LEFT JOIN faces fa ON fa.person_id = p.id AND fa.dismissed = 0
             GROUP BY p.id HAVING n > 0 OR p.immich_person_id IS NOT NULL
             ORDER BY p.name COLLATE NOCASE, p.id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(PersonRow {
                id: r.get(0)?,
                name: r.get(1)?,
                immich_id: r.get(2)?,
                photos: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// The person called `name`, made if there is none. ⚠ It joins the ONE Immich person of that name when exactly
    /// one exists; with two Immich "Anna"s it is a person of its own (the spec's rule).
    pub fn person_named(&self, name: &str) -> Result<i64, CatalogError> {
        let name = name.trim();
        let ids: Vec<(i64, Option<String>)> = {
            let mut st = self
                .conn
                .prepare("SELECT id, immich_person_id FROM people WHERE name = ?1 COLLATE NOCASE ORDER BY id")?;
            let rows = st.query_map([name], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.filter_map(Result::ok).collect()
        };
        let local = ids.iter().find(|(_, i)| i.is_none()).map(|(id, _)| *id);
        let immich: Vec<i64> = ids
            .iter()
            .filter(|(_, i)| i.is_some())
            .map(|(id, _)| *id)
            .collect();
        if let [one] = immich.as_slice() {
            return Ok(*one);
        }
        if let Some(id) = local {
            return Ok(id);
        }
        self.conn
            .execute("INSERT INTO people (name) VALUES (?1)", [name])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Each face's naming, for an undo.
    pub fn face_states(&self, faces: &[i64]) -> Result<Vec<(i64, FaceState)>, CatalogError> {
        let mut st = self
            .conn
            .prepare_cached("SELECT person_id, confirmed, dismissed FROM faces WHERE id = ?1")?;
        let mut out = Vec::new();
        for &id in faces {
            if let Ok(s) = st.query_row([id], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, i64>(1)? != 0,
                    r.get::<_, i64>(2)? != 0,
                ))
            }) {
                out.push((id, s));
            }
        }
        Ok(out)
    }

    /// Set each face's naming — what naming, confirming, clearing, "not a face" and their undos all write.
    pub fn set_face_states(&self, states: &[(i64, FaceState)]) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        for (id, (person, confirmed, dismissed)) in states {
            tx.execute(
                "UPDATE faces SET person_id = ?2, confirmed = ?3, dismissed = ?4 WHERE id = ?1",
                rusqlite::params![id, person, *confirmed as i64, *dismissed as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// "No, not this person" for each face — never offered as them again.
    pub fn set_rejected(
        &self,
        faces: &[i64],
        person: i64,
        rejected: bool,
    ) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        for f in faces {
            if rejected {
                tx.execute(
                    "INSERT OR IGNORE INTO face_rejections (face_id, person_id) VALUES (?1, ?2)",
                    rusqlite::params![f, person],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM face_rejections WHERE face_id = ?1 AND person_id = ?2",
                    rusqlite::params![f, person],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Immich's named, visible people, by their server id — a rename on the server follows. Returns how many.
    pub fn set_immich_people(&self, people: &[(String, String)]) -> Result<usize, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        for (id, name) in people {
            tx.execute(
                "INSERT INTO people (name, immich_person_id) VALUES (?2, ?1)
                 ON CONFLICT(immich_person_id) DO UPDATE SET name = excluded.name",
                rusqlite::params![id, name],
            )?;
        }
        tx.commit()?;
        Ok(people.len())
    }

    /// `photo`'s faces as Immich has them — every earlier Immich face of it replaced (the server's are not ours to
    /// edit). A face of a person not in [`Catalog::set_immich_people`] (unnamed, hidden) is left out.
    pub fn set_immich_faces(
        &self,
        photo: &Path,
        faces: &[([f32; 4], String)],
    ) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        let tx = self.conn.unchecked_transaction()?;
        let fid: Option<i64> = tx
            .query_row("SELECT id FROM folders WHERE path = ?1", [&folder], |r| {
                r.get(0)
            })
            .ok();
        let Some(fid) = fid else {
            return Ok(());
        };
        tx.execute(
            "DELETE FROM faces WHERE folder_id = ?1 AND file_name = ?2 AND source = 'immich'",
            rusqlite::params![fid, name],
        )?;
        for (b, pid) in faces {
            tx.execute(
                "INSERT INTO faces (folder_id, file_name, x0, y0, x1, y1, person_id, confirmed, source)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, id, 1, 'immich' FROM people WHERE immich_person_id = ?7",
                rusqlite::params![fid, name, b[0], b[1], b[2], b[3], pid],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The photos `person` is in, as `(path, copy 0)` — a catalog-backed source's members.
    pub fn person_members(&self, person: i64) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT DISTINCT f.path, fa.file_name FROM faces fa JOIN folders f ON f.id = fa.folder_id
             WHERE fa.person_id = ?1 AND fa.dismissed = 0",
        )?;
        let rows = st.query_map([person], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?).join(r.get::<_, String>(1)?),
                0,
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;

    fn cat_with(names: &[&str]) -> (Catalog, Vec<PathBuf>) {
        let cat = Catalog::open_in_memory().unwrap();
        let e = entries_in("/shoot", names);
        cat.sync_folder(Path::new("/shoot"), &e).unwrap();
        (cat, e.into_iter().map(|e| e.path).collect())
    }

    fn face(x: f32) -> NewFace {
        let mut e = vec![0.0; 128];
        e[0] = 1.0;
        NewFace {
            bbox: [x, 0.1, x + 0.1, 0.3],
            embedding: e,
        }
    }

    #[test]
    fn faces_round_trip_and_name() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_local_faces(&p[0], &[face(0.1), face(0.5)]).unwrap();
        let f = cat.faces_of(&p[0]).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].embedding.as_ref().unwrap()[0], 1.0);
        assert!(cat.files_without_faces_read(&p).unwrap().is_empty());
        let anna = cat.person_named("Anna").unwrap();
        cat.set_face_states(&[(f[0].id, (Some(anna), true, false))])
            .unwrap();
        let people = cat.people().unwrap();
        assert_eq!((people[0].name.as_str(), people[0].photos), ("Anna", 1));
        assert_eq!(cat.person_members(anna).unwrap(), vec![(p[0].clone(), 0)]);
        assert_eq!(cat.unnamed_local_faces().unwrap().len(), 1);
    }

    /// Review focus 1 at the catalog: a new search never moves a name.
    #[test]
    fn set_local_faces_keeps_named_ones() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_local_faces(&p[0], &[face(0.1), face(0.5)]).unwrap();
        let f = cat.faces_of(&p[0]).unwrap();
        let anna = cat.person_named("Anna").unwrap();
        let named = f[0].id;
        cat.set_face_states(&[(named, (Some(anna), true, false))])
            .unwrap();
        cat.set_local_faces(&p[0], &[face(0.7)]).unwrap();
        let after = cat.faces_of(&p[0]).unwrap();
        assert_eq!(after.len(), 2, "{after:?}");
        assert!(
            after
                .iter()
                .any(|g| g.id == named && g.person == Some(anna)),
            "the named face was replaced: {after:?}"
        );
    }

    #[test]
    fn a_name_joins_the_one_immich_person_of_that_name() {
        let (cat, _) = cat_with(&[]);
        cat.set_immich_people(&[("imm-1".into(), "Anna".into())])
            .unwrap();
        let a = cat.person_named("anna").unwrap();
        let again = cat.person_named("Anna").unwrap();
        assert_eq!(a, again);
        let row: Option<String> = cat
            .conn
            .query_row(
                "SELECT immich_person_id FROM people WHERE id = ?1",
                [a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row.as_deref(), Some("imm-1"));
        // A rename on the server follows by id.
        cat.set_immich_people(&[("imm-1".into(), "Anna K".into())])
            .unwrap();
        let name: String = cat
            .conn
            .query_row("SELECT name FROM people WHERE id = ?1", [a], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Anna K");
    }

    /// An Immich person shows with no photos here yet; a local person with none does not.
    #[test]
    fn the_people_list_holds_immich_people_and_local_ones_with_photos() {
        let (cat, _) = cat_with(&[]);
        cat.set_immich_people(&[("i1".into(), "Anna".into())])
            .unwrap();
        cat.person_named("Nobody").unwrap();
        let people = cat.people().unwrap();
        assert_eq!(people.len(), 1, "{people:?}");
        assert_eq!((people[0].name.as_str(), people[0].photos), ("Anna", 0));
    }

    #[test]
    fn two_immich_annas_stay_two() {
        let (cat, _) = cat_with(&[]);
        cat.set_immich_people(&[("i1".into(), "Anna".into()), ("i2".into(), "Anna".into())])
            .unwrap();
        let n: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM people", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        let local = cat.person_named("Anna").unwrap();
        let imm: Option<String> = cat
            .conn
            .query_row(
                "SELECT immich_person_id FROM people WHERE id = ?1",
                [local],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(imm, None, "an ambiguous name is a person of its own");
    }

    #[test]
    fn immich_faces_replace_and_skip_unknown_people() {
        let (cat, p) = cat_with(&["a.JPG"]);
        cat.set_immich_people(&[("i1".into(), "Ben".into())])
            .unwrap();
        let b = [0.1, 0.1, 0.2, 0.2];
        cat.set_immich_faces(&p[0], &[(b, "i1".into()), (b, "unknown".into())])
            .unwrap();
        cat.set_immich_faces(&p[0], &[(b, "i1".into())]).unwrap();
        let f = cat.faces_of(&p[0]).unwrap();
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].immich && f[0].confirmed && f[0].embedding.is_none());
    }

    #[test]
    fn a_dismissed_face_leaves_every_list_and_its_undo_brings_it_back() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_local_faces(&p[0], &[face(0.1)]).unwrap();
        let id = cat.faces_of(&p[0]).unwrap()[0].id;
        let before = cat.face_states(&[id]).unwrap();
        cat.set_face_states(&[(id, (None, false, true))]).unwrap();
        assert!(
            cat.faces_of(&p[0]).unwrap().is_empty()
                && cat.unnamed_local_faces().unwrap().is_empty()
        );
        cat.set_local_faces(&p[0], &[]).unwrap();
        cat.set_face_states(&before).unwrap();
        assert_eq!(
            cat.faces_of(&p[0]).unwrap().len(),
            1,
            "a re-search deleted the dismissed face"
        );
    }

    #[test]
    fn rejections_are_kept_and_undone() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.set_local_faces(&p[0], &[face(0.1)]).unwrap();
        let id = cat.faces_of(&p[0]).unwrap()[0].id;
        let ben = cat.person_named("Ben").unwrap();
        cat.set_rejected(&[id], ben, true).unwrap();
        assert!(cat.face_rejections().unwrap().contains(&(id, ben)));
        cat.set_rejected(&[id], ben, false).unwrap();
        assert!(cat.face_rejections().unwrap().is_empty());
    }

    /// Review focus 4: a move carries the faces and the read mark.
    #[test]
    fn a_moved_photo_keeps_its_faces() {
        let (cat, p) = cat_with(&["a.RAF"]);
        cat.sync_folder(Path::new("/other"), &[]).unwrap();
        cat.set_local_faces(&p[0], &[face(0.1)]).unwrap();
        let to = PathBuf::from("/other/a.RAF");
        cat.move_photo(&p[0], &to).unwrap();
        assert_eq!(cat.faces_of(&to).unwrap().len(), 1);
        assert!(cat
            .files_without_faces_read(std::slice::from_ref(&to))
            .unwrap()
            .is_empty());
    }
}
