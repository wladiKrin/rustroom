//! Forgetting a folder, and restoring what a deletion took — the two halves of one snapshot.
//!
//! ⚠ Together because `forget_folder`'s SELECT column lists must mirror the Folder arm's INSERT
//! lists in `restore`: a column added to one and not the other is a silent loss on undo.

use super::{Catalog, CatalogError, Marks};
use crate::snapshot::{CaptureSnapshot, CatalogSnapshot, DescriptionSnapshot, PhotoSnapshot};
use std::path::PathBuf;

impl Catalog {
    /// Remove a folder from the catalog.
    ///
    /// ⚠⚠ **This deletes catalog rows and never touches a file.** The photos, their sidecars and
    /// every edit in them are untouched — re-opening the folder catalogues it again, minus the
    /// library metadata. That is the M4.1 split doing its job: *delete the catalog and you lose
    /// organisation, never work.*
    ///
    /// ⚠ Its ratings, flags, labels and keyword assignments **do** go, by a chained cascade:
    /// `folders` → `photos` → `collection_photos`, and `folders` → `photo_keywords`. The second
    /// hop needs `PRAGMA foreign_keys = ON`, which is set per connection in `open`.
    /// ⚠ It returns everything it removed, so the deletion can be undone. The read happens
    /// **before** the delete for the obvious reason, and the cascade is what makes the snapshot
    /// closed under references — every row it takes is in here.
    pub fn forget_folder(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let path: String =
            tx.query_row("SELECT path FROM folders WHERE id = ?1", [id], |r| r.get(0))?;

        let mut st = tx.prepare(
            "SELECT id, file_name, copy, size, mtime, last_seen, rating, flag, label
             FROM photos WHERE folder_id = ?1",
        )?;
        let photos: Vec<PhotoSnapshot> = st
            .query_map([id], |r| {
                Ok(PhotoSnapshot {
                    id: r.get(0)?,
                    file_name: r.get(1)?,
                    copy: r.get::<_, i64>(2)? as u32,
                    size: r.get(3)?,
                    mtime: r.get(4)?,
                    last_seen: r.get(5)?,
                    rating: r.get::<_, i64>(6)?.clamp(0, Marks::MAX_RATING as i64) as u8,
                    flag: r.get(7)?,
                    label: r.get::<_, i64>(8)?.clamp(0, Marks::MAX_LABEL as i64) as u8,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT file_name, taken, camera, lens, iso, shutter, aperture, focal_len
             FROM photo_capture WHERE folder_id = ?1",
        )?;
        let captures: Vec<CaptureSnapshot> = st
            .query_map([id], |r| {
                Ok(CaptureSnapshot {
                    file_name: r.get(0)?,
                    taken: r.get(1)?,
                    camera: r.get(2)?,
                    lens: r.get(3)?,
                    iso: r.get(4)?,
                    shutter: r.get(5)?,
                    aperture: r.get(6)?,
                    focal_len: r.get(7)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT file_name, title, caption, copyright, creator
             FROM photo_description WHERE folder_id = ?1",
        )?;
        let descriptions: Vec<DescriptionSnapshot> = st
            .query_map([id], |r| {
                Ok(DescriptionSnapshot {
                    file_name: r.get(0)?,
                    title: r.get(1)?,
                    caption: r.get(2)?,
                    copyright: r.get(3)?,
                    creator: r.get(4)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT cp.collection_id, cp.photo_id, cp.added
             FROM collection_photos cp JOIN photos p ON p.id = cp.photo_id
             WHERE p.folder_id = ?1",
        )?;
        let memberships: Vec<(i64, i64, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st =
            tx.prepare("SELECT file_name, keyword_id FROM photo_keywords WHERE folder_id = ?1")?;
        let keywords: Vec<(String, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT file_name, stack_id, ord, by_time FROM photo_stacks WHERE folder_id = ?1",
        )?;
        let stacks: Vec<(String, crate::db::HandMember)> = st
            .query_map([id], |r| {
                Ok((
                    r.get(0)?,
                    crate::db::HandMember {
                        stack: r.get(1)?,
                        ord: r.get(2)?,
                        by_time: r.get::<_, i64>(3)? != 0,
                    },
                ))
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Folder {
            path: PathBuf::from(path),
            photos,
            captures,
            descriptions,
            memberships,
            keywords,
            stacks,
        })
    }

    /// Put back what a deletion removed.
    ///
    /// ⚠⚠ **Everything is re-inserted with FRESH ids, and the old ones only match children to
    /// parents.** Preserving them collides: SQLite hands a freed rowid to the next insert, so
    /// deleting a collection, creating another and then undoing fails on the UNIQUE index. Because
    /// a cascade removes exactly the dependent rows, a snapshot is closed under references, so
    /// remapping is total.
    ///
    /// ⚠ One transaction, so a restore is all or nothing — a half-restored folder would be worse
    /// than one that failed.
    ///
    /// ⚠ A row whose *other* end has since gone — a membership whose collection was deleted after
    /// the folder was forgotten — is **skipped, not fatal**. The rest still lands, which is the
    /// rule `add_to_collection` already states: the catalog is never a reason an action fails.
    pub fn restore(&self, snap: &CatalogSnapshot) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        match snap {
            CatalogSnapshot::Smart {
                name,
                rule,
                created,
            } => {
                tx.execute(
                    "INSERT INTO smart_collections (name, rule, created) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, rule, created],
                )?;
            }
            CatalogSnapshot::Collection {
                name,
                created,
                members,
            } => {
                tx.execute(
                    "INSERT INTO collections (name, created) VALUES (?1, ?2)",
                    rusqlite::params![name, created],
                )?;
                let cid = tx.last_insert_rowid();
                for (photo_id, added) in members {
                    // ⚠ The photo ids are **not** remapped — the photos were never deleted. One
                    // that has since gone is skipped by the `SELECT ... WHERE EXISTS`.
                    tx.execute(
                        "INSERT INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM photos WHERE id = ?2)",
                        rusqlite::params![cid, photo_id, added],
                    )?;
                }
            }
            CatalogSnapshot::Keyword { tree, assignments } => {
                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                // ⚠ `tree` is parents-first, so a child's remapped parent always already exists.
                for k in tree {
                    let parent = k.parent_id.map(|p| map.get(&p).copied().unwrap_or(p));
                    tx.execute(
                        "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)",
                        rusqlite::params![k.name, parent],
                    )?;
                    map.insert(k.id, tx.last_insert_rowid());
                }
                for (folder_id, file_name, keyword_id) in assignments {
                    let Some(kid) = map.get(keyword_id) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM folders WHERE id = ?1)",
                        rusqlite::params![folder_id, file_name, kid],
                    )?;
                }
            }
            CatalogSnapshot::Folder {
                path,
                photos,
                captures,
                descriptions,
                memberships,
                keywords,
                stacks,
            } => {
                let path = path.to_string_lossy().to_string();
                let last_seen = photos.iter().map(|p| p.last_seen).max().unwrap_or(0);
                tx.execute(
                    "INSERT INTO folders (path, last_seen) VALUES (?1, ?2)
                     ON CONFLICT(path) DO UPDATE SET last_seen = ?2",
                    rusqlite::params![path, last_seen],
                )?;
                let fid: i64 =
                    tx.query_row("SELECT id FROM folders WHERE path = ?1", [&path], |r| {
                        r.get(0)
                    })?;

                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                for p in photos {
                    tx.execute(
                        "INSERT INTO photos
                             (folder_id, file_name, copy, size, mtime, last_seen, rating, flag, label)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(folder_id, file_name, copy) DO UPDATE SET
                             size = ?4, mtime = ?5, last_seen = ?6,
                             rating = ?7, flag = ?8, label = ?9",
                        rusqlite::params![
                            fid,
                            p.file_name,
                            p.copy,
                            p.size,
                            p.mtime,
                            p.last_seen,
                            p.rating,
                            p.flag,
                            p.label,
                        ],
                    )?;
                    let nid: i64 = tx.query_row(
                        "SELECT id FROM photos WHERE folder_id = ?1 AND file_name = ?2 AND copy = ?3",
                        rusqlite::params![fid, p.file_name, p.copy],
                        |r| r.get(0),
                    )?;
                    map.insert(p.id, nid);
                }

                for c in captures {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_capture
                             (folder_id, file_name, taken, camera, lens, iso, shutter, aperture, focal_len)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            fid,
                            c.file_name,
                            c.taken,
                            c.camera,
                            c.lens,
                            c.iso,
                            c.shutter,
                            c.aperture,
                            c.focal_len,
                        ],
                    )?;
                }

                for d in descriptions {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_description
                             (folder_id, file_name, title, caption, copyright, creator)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            fid,
                            d.file_name,
                            d.title,
                            d.caption,
                            d.copyright,
                            d.creator
                        ],
                    )?;
                }

                for (collection_id, photo_id, added) in memberships {
                    let Some(pid) = map.get(photo_id) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (SELECT 1 FROM collections WHERE id = ?1)",
                        rusqlite::params![collection_id, pid, added],
                    )?;
                }

                for (file_name, keyword_id) in keywords {
                    tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (SELECT 1 FROM keywords WHERE id = ?3)",
                        rusqlite::params![fid, file_name, keyword_id],
                    )?;
                }

                for (file_name, m) in stacks {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_stacks
                             (folder_id, file_name, stack_id, ord, by_time)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![fid, file_name, m.stack, m.ord, m.by_time as i64],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{entries_in, TempDb};
    use crate::db::{Description, Flag};
    use crate::CatalogEntry;
    use core_image::Capture;
    use std::path::Path;

    #[test]
    fn forgetting_a_folder_cascades_through_photos_to_collections_and_keywords() {
        // ⚠⚠ A **chained** cascade, and the first feature to depend on two hops in series:
        // folders -> photos -> collection_photos, and folders -> photo_keywords directly. The
        // second hop happens only because `PRAGMA foreign_keys = ON` is set per connection —
        // SQLite defaults it OFF, which is why that pragma already has a test of its own.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"]))
            .unwrap();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, Path::new("/a/one.RAF"), 0)
            .unwrap();
        cat.add_to_collection(coll, Path::new("/b/two.RAF"), 0)
            .unwrap();
        let kw = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(kw, Path::new("/a/one.RAF")).unwrap();

        let id = cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == Path::new("/a"))
            .unwrap()
            .id;
        cat.forget_folder(id).unwrap();

        // ⚠⚠ THE ASSERTIONS THAT MATTER ARE THE **RAW COUNTS**, AND FINDING THAT OUT COST A
        // SABOTAGE. `all_photos`, `collection_members` and `keyword_members` all JOIN through
        // `folders` — so with the cascade disabled the orphaned rows simply drop out of those
        // queries and every join-based assertion below passes vacuously. The first version of
        // this test asserted only on those, and `PRAGMA foreign_keys = OFF` left it green.
        //
        // `photo_count`, and the counts inside `collections()` and `keywords()`, do **not** join
        // `folders` — they count the rows themselves, which is exactly what a missing cascade
        // leaves behind. Orphans are invisible to the UI and accumulate forever, and worse: a
        // folder re-added at the same path gets a *new* id, so the old orphans are unreachable.
        assert_eq!(
            cat.photo_count().unwrap(),
            1,
            "orphaned photo rows survived"
        );
        assert_eq!(
            cat.collections().unwrap()[0].count,
            1,
            "an orphaned collection membership survived"
        );
        assert_eq!(
            cat.keywords().unwrap()[0].count,
            0,
            "an orphaned keyword assignment survived"
        );

        // And the user-visible effect, which the raw counts do not by themselves prove.
        assert_eq!(cat.all_photos().unwrap().len(), 1, "photo rows survived");
        assert_eq!(cat.folders().unwrap().len(), 1, "the folder row survived");
        assert_eq!(
            cat.collection_members(coll).unwrap().len(),
            1,
            "the membership of the forgotten folder's photo survived"
        );
        assert!(
            cat.keyword_members(kw).unwrap().is_empty(),
            "the keyword assignment survived"
        );
    }

    #[test]
    fn forgetting_a_folder_leaves_the_other_folders_alone() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"]))
            .unwrap();
        let id = cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == Path::new("/a"))
            .unwrap()
            .id;
        cat.forget_folder(id).unwrap();
        let left = cat.all_photos().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, Path::new("/b/two.RAF"));
    }

    #[test]
    fn forgetting_a_folder_deletes_no_files() {
        // ⚠⚠ The M4.1 split, asserted: *delete the catalog and you lose organisation, never work.*
        // Edits live in the per-photo sidecars and survive this entirely. Task 1's in-memory tests
        // structurally cannot see this — they have no files behind them.
        let db = TempDb::new("nofiles");
        let dir = db.0.parent().unwrap().to_path_buf();
        let photo = dir.join("one.RAF");
        std::fs::write(&photo, b"not really a raw").unwrap();
        let sidecar = dir.join("one.RAF.json");
        std::fs::write(&sidecar, b"{}").unwrap();

        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            &dir,
            &[CatalogEntry {
                path: photo.clone(),
                file_name: "one.RAF".to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();

        assert!(photo.exists(), "the photo was deleted");
        assert!(sidecar.exists(), "the sidecar was deleted");
        assert!(cat.all_photos().unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_folder_cascades_to_its_capture_rows() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(
            Path::new("/a/one.RAF"),
            &Capture {
                iso: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();
        // ⚠ The **raw** count, not a join — the lesson from `forget_folder`'s own cascade test,
        // where every join-based assertion passed vacuously with the cascade disabled.
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_capture", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "orphaned capture rows survived");
    }

    /// A catalog with one folder of two photos, for the restore tests.
    fn with_two_photos() -> (Catalog, PathBuf) {
        let cat = Catalog::open_in_memory().unwrap();
        let dir = PathBuf::from("/a");
        cat.sync_folder(&dir, &entries_in("/a", &["a.RAF", "b.RAF"]))
            .unwrap();
        (cat, dir)
    }

    /// ⚠⚠ **The collision that motivated the whole design.** Delete a collection, create another
    /// (which takes the freed id, because SQLite reuses `max + 1` without `AUTOINCREMENT`), then
    /// undo. Preserving the original id fails here with `UNIQUE constraint failed`; remapping
    /// cannot.
    #[test]
    fn a_restore_survives_its_id_being_taken_by_something_else() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_collection("Portfolio").unwrap();
        let snap = cat.delete_collection(id).unwrap();
        let other = cat.create_collection("Travel").unwrap();
        assert_eq!(other, id, "fixture check: the freed id really was reused");

        cat.restore(&snap).unwrap();
        let names: Vec<String> = cat
            .collections()
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.contains(&"Portfolio".to_string()));
        assert!(names.contains(&"Travel".to_string()));
    }

    /// ⚠ The marks are the point of a folder restore: they are the only thing a forget destroys
    /// that the user made by hand, and a restore that dropped them would look like it worked.
    #[test]
    fn forgetting_a_folder_and_undoing_brings_back_its_marks() {
        let (cat, dir) = with_two_photos();
        cat.set_marks(
            &dir.join("a.RAF"),
            0,
            Marks {
                rating: 4,
                flag: Flag::Pick,
                label: 2,
            },
        )
        .unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 0);

        cat.restore(&snap).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2);
        let back = cat.marks_in(&dir).unwrap();
        let (_, _, m) = back
            .iter()
            .find(|(_, _, m)| m.rating == 4)
            .expect("the rating must come back");
        assert_eq!(m.flag, Flag::Pick);
        assert_eq!(m.label, 2);
    }

    #[test]
    fn forgetting_a_folder_and_undoing_brings_back_its_descriptions() {
        // ⚠ Same sentence as the marks test, one more table: a restore that dropped the
        // captions would look like it had worked.
        let (cat, dir) = with_two_photos();
        let d = Description {
            title: "Kept".into(),
            caption: "through a forget and an undo".into(),
            ..Description::default()
        };
        cat.set_description(&dir.join("a.RAF"), &d).unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_description", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "orphaned description rows survived the forget");

        cat.restore(&snap).unwrap();
        assert_eq!(cat.description_of(&dir.join("a.RAF")).unwrap(), Some(d));
        assert_eq!(cat.description_of(&dir.join("b.RAF")).unwrap(), None);
    }

    /// ⚠ A keyword takes its whole subtree, so the restore must bring the hierarchy back — not
    /// just the rows, but the parent links, which are remapped.
    #[test]
    fn undoing_a_keyword_deletion_restores_the_subtree_and_its_shape() {
        let cat = Catalog::open_in_memory().unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let france = cat.create_keyword("France", Some(europe)).unwrap();
        cat.create_keyword("Paris", Some(france)).unwrap();

        let snap = cat.delete_keyword(europe).unwrap();
        assert!(cat.keywords().unwrap().is_empty());

        cat.restore(&snap).unwrap();
        let rows = cat.keywords().unwrap();
        assert_eq!(rows.len(), 3, "all three come back");
        let by_name = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
        let (e, f, p) = (by_name("Europe"), by_name("France"), by_name("Paris"));
        assert_eq!(e.parent_id, None, "Europe is still a root");
        assert_eq!(f.parent_id, Some(e.id), "France still hangs off Europe");
        assert_eq!(p.parent_id, Some(f.id), "and Paris off France");
    }

    /// ⚠⚠ **A row that can no longer be satisfied is skipped, not fatal.** The rest of the restore
    /// must still land — the rule `add_to_collection` already states, that the catalog must never
    /// be a reason an action fails. Asserted by *count*, so "it did not error" cannot pass it.
    #[test]
    fn a_membership_whose_collection_is_gone_is_skipped_and_the_rest_restores() {
        let (cat, dir) = with_two_photos();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        // The collection those memberships pointed at is deleted before the undo.
        cat.delete_collection(coll).unwrap();

        cat.restore(&snap).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2, "the photos still come back");
    }

    #[test]
    fn undoing_a_smart_collection_deletion_restores_its_rule() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Picks", "{\"All\":[]}").unwrap();
        let snap = cat.delete_smart(id).unwrap();
        assert!(cat.smart_collections().unwrap().is_empty());

        cat.restore(&snap).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Picks");
        assert_eq!(rows[0].rule, "{\"All\":[]}");
    }

    /// ⚠ A collection's members must come back, not just its name.
    #[test]
    fn undoing_a_collection_deletion_restores_its_members() {
        let (cat, dir) = with_two_photos();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();

        let snap = cat.delete_collection(coll).unwrap();
        cat.restore(&snap).unwrap();

        let row = cat.collections().unwrap().into_iter().next().unwrap();
        assert_eq!(row.name, "Portfolio");
        assert_eq!(row.count, 1, "the membership came back too");
    }
}
