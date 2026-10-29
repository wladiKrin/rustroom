//! Publish services (M10.18d): a named destination — a folder or an Immich album — with export settings,
//! the photos (copies) it holds, and what was last published of each.
//!
//! ⚠ Keyed on `photos.id`, as `collection_photos` is: a move re-keys the `photos` row and its id rides
//! along, so these tables need no place in `PER_FILE_TABLES`. Forget/restore carries them (`snapshot` /
//! `restore` below, called from `restore.rs`).

use super::{unix_now, Catalog, CatalogError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a service publishes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishTarget {
    Folder {
        path: PathBuf,
    },
    /// An Immich album: `album_id` is `None` until the first publish creates it under `album_name`.
    Immich {
        album_id: Option<String>,
        album_name: String,
    },
}

impl PublishTarget {
    fn kind(&self) -> &'static str {
        match self {
            PublishTarget::Folder { .. } => "folder",
            PublishTarget::Immich { .. } => "immich",
        }
    }
}

/// A service, with how many members it holds (removed ones not counted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishServiceRow {
    pub id: i64,
    pub name: String,
    pub target: PublishTarget,
    /// The export settings, a snapshot (`ExportOptions` as JSON — the catalog does not know the type).
    pub options_json: String,
    pub count: i64,
}

/// Where one published copy is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    /// A file the service wrote, with its size and mtime then — a removal deletes it only if both still match.
    File {
        path: PathBuf,
        size: u64,
        mtime: i64,
    },
    /// An Immich asset.
    Asset { id: String },
}

/// What was last published of a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub fingerprint: u64,
    pub place: Place,
    pub at: i64,
}

/// One member of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub path: PathBuf,
    pub copy: u32,
    /// Taken out of the service, its published copy still at the destination.
    pub removed: bool,
    pub published: Option<Published>,
}

/// A forgotten folder's publish rows: `(service, photo, added, removed)` and
/// `(service, photo, fingerprint, place JSON, at)` — the photo id is remapped on restore, the service id not.
pub type PublishSnapshot = (
    Vec<(i64, i64, i64, bool)>,
    Vec<(i64, i64, i64, String, i64)>,
);

fn target_json(t: &PublishTarget) -> String {
    serde_json::to_string(t).unwrap_or_default()
}

fn no_service() -> CatalogError {
    CatalogError::Sql(rusqlite::Error::QueryReturnedNoRows)
}

impl Catalog {
    /// Make a service. ⚠ A taken name is an ERROR, unlike a collection's: a service is made in a dialog,
    /// which says so, and two services of one name could not be told apart in the rail.
    pub fn create_publish_service(
        &self,
        name: &str,
        target: &PublishTarget,
        options_json: &str,
    ) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO publish_services (name, kind, target, options, created)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                name,
                target.kind(),
                target_json(target),
                options_json,
                unix_now()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Every service, name-ordered case-insensitively.
    pub fn publish_services(&self) -> Result<Vec<PublishServiceRow>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT s.id, s.name, s.target, s.options,
                    (SELECT count(*) FROM publish_members m WHERE m.service_id = s.id AND m.removed = 0)
             FROM publish_services s ORDER BY lower(s.name), s.name",
        )?;
        let rows = st.query_map([], |r| {
            let target: String = r.get(2)?;
            Ok((r.get(0)?, r.get(1)?, target, r.get(3)?, r.get(4)?))
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|(id, name, target, options_json, count)| {
                Some(PublishServiceRow {
                    id,
                    name,
                    target: serde_json::from_str(&target).ok()?,
                    options_json,
                    count,
                })
            })
            .collect())
    }

    pub fn update_publish_service(
        &self,
        id: i64,
        name: &str,
        target: &PublishTarget,
        options_json: &str,
    ) -> Result<(), CatalogError> {
        let n = self.conn.execute(
            "UPDATE publish_services SET name = ?2, kind = ?3, target = ?4, options = ?5 WHERE id = ?1",
            rusqlite::params![id, name, target.kind(), target_json(target), options_json],
        )?;
        if n == 0 {
            return Err(no_service());
        }
        Ok(())
    }

    /// Remove a service and its rows. ⚠ Nothing it published is touched — that is the caller's promise too.
    pub fn delete_publish_service(&self, id: i64) -> Result<(), CatalogError> {
        self.conn
            .execute("DELETE FROM publish_services WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Add a copy; whether anything changed. A removed member comes back (its published row kept, so it
    /// reads Published or Modified again rather than New).
    pub fn add_to_service(&self, id: i64, photo: &Path, copy: u32) -> Result<bool, CatalogError> {
        let Some(pid) = self.photo_id(photo, copy)? else {
            return Ok(false);
        };
        Ok(self.conn.execute(
            "INSERT INTO publish_members (service_id, photo_id, added, removed) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(service_id, photo_id) DO UPDATE SET removed = 0 WHERE removed = 1",
            rusqlite::params![id, pid, unix_now()],
        )? > 0)
    }

    /// Take a copy out: one ever published stays as Removed until a publish clears it; one never
    /// published simply goes.
    pub fn remove_from_service(
        &self,
        id: i64,
        photo: &Path,
        copy: u32,
    ) -> Result<bool, CatalogError> {
        let Some(pid) = self.photo_id(photo, copy)? else {
            return Ok(false);
        };
        let tx = self.conn.unchecked_transaction()?;
        let published: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM published WHERE service_id = ?1 AND photo_id = ?2)",
            [id, pid],
            |r| r.get(0),
        )?;
        let n = if published {
            tx.execute(
                "UPDATE publish_members SET removed = 1
                 WHERE service_id = ?1 AND photo_id = ?2 AND removed = 0",
                [id, pid],
            )?
        } else {
            tx.execute(
                "DELETE FROM publish_members WHERE service_id = ?1 AND photo_id = ?2",
                [id, pid],
            )?
        };
        tx.commit()?;
        Ok(n > 0)
    }

    /// Every member, removed ones included, path-ordered.
    pub fn publish_members(&self, id: i64) -> Result<Vec<Member>, CatalogError> {
        let mut st = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy, m.removed, pb.fingerprint, pb.place, pb.at
             FROM publish_members m
             JOIN photos p  ON p.id = m.photo_id
             JOIN folders f ON f.id = p.folder_id
             LEFT JOIN published pb ON pb.service_id = m.service_id AND pb.photo_id = m.photo_id
             WHERE m.service_id = ?1
             ORDER BY lower(f.path), lower(p.file_name), p.copy",
        )?;
        let rows = st.query_map([id], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            let print: Option<i64> = r.get(4)?;
            let place: Option<String> = r.get(5)?;
            let at: Option<i64> = r.get(6)?;
            let published = match (print, place.and_then(|p| serde_json::from_str(&p).ok()), at) {
                (Some(f), Some(place), Some(at)) => Some(Published {
                    fingerprint: f as u64,
                    place,
                    at,
                }),
                _ => None,
            };
            Ok(Member {
                path: PathBuf::from(folder).join(name),
                copy: r.get(2)?,
                removed: r.get(3)?,
                published,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Record what a publish wrote for a member (replacing what was there).
    pub fn record_published(
        &self,
        id: i64,
        photo: &Path,
        copy: u32,
        p: &Published,
    ) -> Result<(), CatalogError> {
        let Some(pid) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO published (service_id, photo_id, fingerprint, place, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                pid,
                p.fingerprint as i64,
                serde_json::to_string(&p.place).unwrap_or_default(),
                p.at
            ],
        )?;
        Ok(())
    }

    /// A Removed member cleared from its destination: its rows go.
    pub fn clear_member(&self, id: i64, photo: &Path, copy: u32) -> Result<(), CatalogError> {
        let Some(pid) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM published WHERE service_id = ?1 AND photo_id = ?2",
            [id, pid],
        )?;
        tx.execute(
            "DELETE FROM publish_members WHERE service_id = ?1 AND photo_id = ?2",
            [id, pid],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// The publish rows of folder `folder_id`'s photos, for a forget's snapshot.
pub(super) fn snapshot(
    tx: &rusqlite::Transaction,
    folder_id: i64,
) -> Result<PublishSnapshot, CatalogError> {
    let mut st = tx.prepare(
        "SELECT m.service_id, m.photo_id, m.added, m.removed
         FROM publish_members m JOIN photos p ON p.id = m.photo_id WHERE p.folder_id = ?1",
    )?;
    let members = st
        .query_map([folder_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .filter_map(Result::ok)
        .collect();
    drop(st);
    let mut st = tx.prepare(
        "SELECT b.service_id, b.photo_id, b.fingerprint, b.place, b.at
         FROM published b JOIN photos p ON p.id = b.photo_id WHERE p.folder_id = ?1",
    )?;
    let published = st
        .query_map([folder_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .filter_map(Result::ok)
        .collect();
    drop(st);
    Ok((members, published))
}

/// Put a snapshot's publish rows back, the photo ids remapped through `map`; a row whose service has since
/// gone is skipped (the restore's rule).
pub(super) fn restore(
    tx: &rusqlite::Transaction,
    snap: &PublishSnapshot,
    map: &HashMap<i64, i64>,
) -> Result<(), CatalogError> {
    for (sid, photo, added, removed) in &snap.0 {
        let Some(pid) = map.get(photo) else { continue };
        tx.execute(
            "INSERT OR IGNORE INTO publish_members (service_id, photo_id, added, removed)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM publish_services WHERE id = ?1)",
            rusqlite::params![sid, pid, added, removed],
        )?;
    }
    for (sid, photo, print, place, at) in &snap.1 {
        let Some(pid) = map.get(photo) else { continue };
        tx.execute(
            "INSERT OR IGNORE INTO published (service_id, photo_id, fingerprint, place, at)
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM publish_services WHERE id = ?1)",
            rusqlite::params![sid, pid, print, place, at],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;

    fn with_two() -> (Catalog, PathBuf) {
        let cat = Catalog::open_in_memory().unwrap();
        let dir = PathBuf::from("/a");
        cat.sync_folder(&dir, &entries_in("/a", &["a.RAF", "b.RAF"]))
            .unwrap();
        (cat, dir)
    }

    fn folder() -> PublishTarget {
        PublishTarget::Folder {
            path: "/out".into(),
        }
    }

    fn file(at: &str) -> Published {
        Published {
            fingerprint: 7,
            place: Place::File {
                path: at.into(),
                size: 10,
                mtime: 20,
            },
            at: 30,
        }
    }

    #[test]
    fn a_service_round_trips_its_target_and_options() {
        let (cat, _) = with_two();
        let t = PublishTarget::Immich {
            album_id: None,
            album_name: "Web".into(),
        };
        let id = cat
            .create_publish_service("Web", &t, r#"{"quality":80}"#)
            .unwrap();
        let rows = cat.publish_services().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].target, t);
        assert_eq!(rows[0].options_json, r#"{"quality":80}"#);
        let t2 = PublishTarget::Immich {
            album_id: Some("abc".into()),
            album_name: "Web".into(),
        };
        cat.update_publish_service(id, "Website", &t2, "{}")
            .unwrap();
        let rows = cat.publish_services().unwrap();
        assert_eq!((rows[0].name.as_str(), &rows[0].target), ("Website", &t2));
    }

    #[test]
    fn a_taken_name_is_refused() {
        let (cat, _) = with_two();
        cat.create_publish_service("Phone", &folder(), "{}")
            .unwrap();
        assert!(cat
            .create_publish_service("phone", &folder(), "{}")
            .is_err());
    }

    #[test]
    fn removing_a_published_member_marks_it_removed_and_an_unpublished_one_goes() {
        let (cat, dir) = with_two();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        let (a, b) = (dir.join("a.RAF"), dir.join("b.RAF"));
        assert!(cat.add_to_service(s, &a, 0).unwrap());
        assert!(cat.add_to_service(s, &b, 0).unwrap());
        cat.record_published(s, &a, 0, &file("/out/a.jpg")).unwrap();
        assert!(cat.remove_from_service(s, &a, 0).unwrap());
        assert!(cat.remove_from_service(s, &b, 0).unwrap());
        let m = cat.publish_members(s).unwrap();
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].removed && m[0].path == a);
        assert_eq!(cat.publish_services().unwrap()[0].count, 0);
    }

    #[test]
    fn readding_a_removed_member_clears_removed() {
        let (cat, dir) = with_two();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        let a = dir.join("a.RAF");
        cat.add_to_service(s, &a, 0).unwrap();
        cat.record_published(s, &a, 0, &file("/out/a.jpg")).unwrap();
        cat.remove_from_service(s, &a, 0).unwrap();
        assert!(cat.add_to_service(s, &a, 0).unwrap());
        assert!(
            !cat.add_to_service(s, &a, 0).unwrap(),
            "a second add changed something"
        );
        let m = cat.publish_members(s).unwrap();
        assert!(!m[0].removed);
        assert_eq!(
            m[0].published,
            Some(file("/out/a.jpg")),
            "the published row was lost"
        );
    }

    #[test]
    fn record_published_upserts_and_a_u64_fingerprint_survives() {
        let (cat, dir) = with_two();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        let a = dir.join("a.RAF");
        cat.add_to_service(s, &a, 0).unwrap();
        cat.record_published(s, &a, 0, &file("/out/a.jpg")).unwrap();
        let p = Published {
            fingerprint: u64::MAX - 3,
            place: Place::Asset { id: "x1".into() },
            at: 5,
        };
        cat.record_published(s, &a, 0, &p).unwrap();
        assert_eq!(cat.publish_members(s).unwrap()[0].published, Some(p));
        cat.clear_member(s, &a, 0).unwrap();
        assert!(cat.publish_members(s).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_service_keeps_the_photos() {
        let (cat, dir) = with_two();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        cat.add_to_service(s, &dir.join("a.RAF"), 0).unwrap();
        cat.delete_publish_service(s).unwrap();
        assert!(cat.publish_services().unwrap().is_empty());
        assert!(cat.publish_members(s).unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 2);
    }

    #[test]
    fn a_moved_photo_keeps_its_published_row() {
        let (cat, dir) = with_two();
        cat.sync_folder(Path::new("/b"), &[]).unwrap();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        let a = dir.join("a.RAF");
        cat.add_to_service(s, &a, 0).unwrap();
        cat.record_published(s, &a, 0, &file("/out/a.jpg")).unwrap();
        cat.move_photo(&a, Path::new("/b/a.RAF")).unwrap();
        let m = cat.publish_members(s).unwrap();
        assert_eq!(m[0].path, PathBuf::from("/b/a.RAF"));
        assert_eq!(m[0].published, Some(file("/out/a.jpg")));
    }

    #[test]
    fn a_forgotten_folder_restores_its_publish_rows() {
        let (cat, dir) = with_two();
        let s = cat.create_publish_service("S", &folder(), "{}").unwrap();
        let (a, b) = (dir.join("a.RAF"), dir.join("b.RAF"));
        cat.add_to_service(s, &a, 0).unwrap();
        cat.add_to_service(s, &b, 0).unwrap();
        cat.record_published(s, &a, 0, &file("/out/a.jpg")).unwrap();
        cat.remove_from_service(s, &a, 0).unwrap();
        let before = cat.publish_members(s).unwrap();
        let fid = cat.folders().unwrap()[0].id;
        let snap = cat.forget_folder(fid).unwrap();
        assert!(cat.publish_members(s).unwrap().is_empty());
        cat.restore(&snap).unwrap();
        assert_eq!(cat.publish_members(s).unwrap(), before);
    }

    #[test]
    fn an_old_catalog_migrates_to_18() {
        let cat = Catalog::open_in_memory().unwrap();
        assert_eq!(cat.version().unwrap(), 18);
        assert!(cat.publish_services().unwrap().is_empty());
    }
}
