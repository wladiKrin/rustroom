//! One Publish (M10.18d): what to export, how a collision is answered, what to record afterwards, and the
//! album and folder bookkeeping. No egui — the bin's `publish_flow` drives it around an `ExportWorker`.
//!
//! ⚠⚠ A member is recorded only once it is DELIVERED: a kept file (folder) or an asset that is in the album
//! (Immich). A failed or unconfirmed photo, or an album step that did not take, leaves the member New or
//! Modified for the next Publish — a re-upload of the same bytes is a `duplicate` with the same asset id.
//! ⚠⚠ Nothing here deletes an Immich asset; a folder file goes only through `publish_folder::clear`.

use crate::batch_export::{Collision, ExportReport};
use crate::publish_folder::{self, Cleared};
use crate::publish_state::{state, State};
use catalog::db::{Catalog, Member, Place, Published};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A member's key: its file and copy.
pub type Key = (PathBuf, u32);

/// What one Publish will do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublishPlan {
    /// New and Modified members, Modified first — so a re-published photo takes back its own file name before
    /// a new one with the same stem could — then in path order.
    pub export: Vec<Key>,
    /// Removed members, to clear from the destination.
    pub clear: Vec<Member>,
    /// What each exported member is published as.
    pub prints: HashMap<Key, u64>,
    /// The files this run replaces — the recorded files of the Modified members.
    pub replacing: HashSet<PathBuf>,
    /// The assets this run replaces, by member.
    pub old_assets: HashMap<Key, String>,
}

/// Plan a Publish of `members`, each non-removed one's current print in `prints` (a member without one is
/// exported — never silently skipped).
pub fn plan(members: &[Member], prints: &HashMap<Key, u64>) -> PublishPlan {
    let mut p = PublishPlan::default();
    let mut new = Vec::new();
    for m in members {
        let key = (m.path.clone(), m.copy);
        let now = prints.get(&key).copied();
        let s = match now {
            Some(f) => state(m, f),
            None if m.removed => State::Removed,
            None => State::New,
        };
        match s {
            State::Removed => p.clear.push(m.clone()),
            State::Published => {}
            State::New | State::Modified => {
                if let Some(f) = now {
                    p.prints.insert(key.clone(), f);
                }
                if s == State::Modified {
                    match m.published.as_ref().map(|x| &x.place) {
                        Some(Place::File { path, .. }) => {
                            p.replacing.insert(path.clone());
                        }
                        Some(Place::Asset { id }) => {
                            p.old_assets.insert(key.clone(), id.clone());
                        }
                        None => {}
                    }
                    p.export.push(key);
                } else {
                    new.push(key);
                }
            }
        }
    }
    p.export.extend(new);
    p
}

/// The answer to the export's collision at `dest`: the service's own file, being re-published, is
/// overwritten; anything else is kept and the new file named beside it.
pub fn collision_answer(dest: &Path, replacing: &HashSet<PathBuf>) -> Collision {
    if replacing.contains(dest) {
        Collision::Overwrite
    } else {
        Collision::KeepBoth
    }
}

/// What a Publish did, for the status row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub published: usize,
    pub failed: usize,
    pub cleared: usize,
    /// Files changed since they were published, left where they are.
    pub left: usize,
    pub cancelled: bool,
    pub notes: Vec<String>,
}

impl Outcome {
    /// `Published 5, 1 failed, removed 1, left 1 file changed since it was published`.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("Published {}", self.published)];
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.cleared > 0 {
            parts.push(format!("removed {}", self.cleared));
        }
        if self.left > 0 {
            parts.push(format!(
                "left {} file{} changed since {} published",
                self.left,
                if self.left == 1 { "" } else { "s" },
                if self.left == 1 {
                    "it was"
                } else {
                    "they were"
                }
            ));
        }
        if self.cancelled {
            parts.push("cancelled".into());
        }
        parts.extend(self.notes.iter().cloned());
        parts.join(", ")
    }
}

/// What the run delivered, as the rows to record: a kept file's place (read now, for the removal check), or a
/// confirmed asset. ⚠ Only members of `plan.export` — the report is the run's, and the run was the plan's.
pub fn delivered(plan: &PublishPlan, report: &ExportReport, now: i64) -> Vec<(Key, Published)> {
    let wanted: HashSet<&Key> = plan.export.iter().collect();
    report
        .written
        .iter()
        .filter_map(|w| {
            let key = (w.id.path.clone(), w.id.copy);
            if !wanted.contains(&key) {
                return None;
            }
            let place = match (&w.asset, &w.file) {
                (Some(id), _) => Place::Asset { id: id.clone() },
                (None, Some(f)) => publish_folder::place_of(f).ok()?,
                (None, None) => return None,
            };
            let fingerprint = plan.prints.get(&key).copied()?;
            Some((
                key,
                Published {
                    fingerprint,
                    place,
                    at: now,
                },
            ))
        })
        .collect()
}

/// Record `rows` for `service`; how many landed.
pub fn record(cat: &Catalog, service: i64, rows: &[(Key, Published)]) -> usize {
    rows.iter()
        .filter(|((path, copy), p)| cat.record_published(service, path, *copy, p).is_ok())
        .count()
}

/// Clear a folder service's Removed members: `(cleared, left)`. A file changed since it was published is
/// left, and its member stays Removed so the user can see it.
pub fn clear_folder(cat: &Catalog, service: i64, members: &[Member]) -> (usize, usize) {
    let (mut cleared, mut left) = (0, 0);
    for m in members {
        let outcome = m
            .published
            .as_ref()
            .map_or(Cleared::Gone, |p| publish_folder::clear(&p.place));
        match outcome {
            Cleared::Deleted | Cleared::Gone => {
                if cat.clear_member(service, &m.path, m.copy).is_ok() {
                    cleared += 1;
                }
            }
            Cleared::Changed => left += 1,
        }
    }
    (cleared, left)
}

/// The album calls a publish makes — [`immich::Client`]'s, a recorder's in tests.
pub trait Albums {
    fn create(&self, name: &str) -> Result<String, immich::Error>;
    fn add(&self, album: &str, ids: &[String]) -> Result<Vec<immich::BulkId>, immich::Error>;
    fn remove(&self, album: &str, ids: &[String]) -> Result<Vec<immich::BulkId>, immich::Error>;
}

impl Albums for immich::Client {
    fn create(&self, name: &str) -> Result<String, immich::Error> {
        self.create_album(name)
    }
    fn add(&self, album: &str, ids: &[String]) -> Result<Vec<immich::BulkId>, immich::Error> {
        self.add_to_album(album, ids)
    }
    fn remove(&self, album: &str, ids: &[String]) -> Result<Vec<immich::BulkId>, immich::Error> {
        self.remove_from_album(album, ids)
    }
}

/// What an album step settled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumSync {
    /// The album's id — new when this step created it.
    pub album: String,
    /// Assets now in the album.
    pub added: HashSet<String>,
    /// Assets now out of it.
    pub removed: HashSet<String>,
}

/// A refusal, said with the permission the Write key lacks.
fn refusal(e: immich::Error, permission: &str) -> String {
    match e {
        immich::Error::Unauthorized => {
            format!("Immich refused: the Write key needs the {permission} permission")
        }
        other => format!("Immich: {other}"),
    }
}

/// Bring the album in step: create it when `album` is `None`, add `add`, then take out each replaced asset
/// whose replacement is in (`replaced`: old → new) and every asset in `drop`. ⚠ Never deletes an asset.
pub fn sync_album(
    albums: &dyn Albums,
    album: Option<&str>,
    name: &str,
    add: &[String],
    replaced: &HashMap<String, String>,
    drop: &[String],
) -> Result<AlbumSync, String> {
    let album = match album {
        Some(a) => a.to_string(),
        None => albums
            .create(name)
            .map_err(|e| refusal(e, "album.create"))?,
    };
    let mut out = AlbumSync {
        album: album.clone(),
        ..Default::default()
    };
    if !add.is_empty() {
        for r in albums
            .add(&album, add)
            .map_err(|e| refusal(e, "albumAsset.create"))?
        {
            if r.settled() {
                out.added.insert(r.id);
            }
        }
    }
    let mut take: Vec<String> = replaced
        .iter()
        .filter(|(old, new)| out.added.contains(*new) && old != new)
        .map(|(old, _)| old.clone())
        .collect();
    take.extend(drop.iter().cloned());
    take.sort();
    take.dedup();
    if !take.is_empty() {
        for r in albums
            .remove(&album, &take)
            .map_err(|e| refusal(e, "albumAsset.delete"))?
        {
            if r.settled() {
                out.removed.insert(r.id);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::WrittenItem;
    use crate::photo_id::PhotoId;
    use std::cell::RefCell;

    fn member(path: &str, published: Option<(u64, Place)>, removed: bool) -> Member {
        Member {
            path: path.into(),
            copy: 0,
            removed,
            published: published.map(|(fingerprint, place)| Published {
                fingerprint,
                place,
                at: 1,
            }),
        }
    }

    fn file(p: &str) -> Place {
        Place::File {
            path: p.into(),
            size: 1,
            mtime: 1,
        }
    }

    fn prints(keys: &[(&str, u64)]) -> HashMap<Key, u64> {
        keys.iter()
            .map(|(p, f)| ((PathBuf::from(p), 0), *f))
            .collect()
    }

    #[test]
    fn the_plan_takes_new_and_modified_and_clears_removed() {
        let ms = [
            member("/a/new.RAF", None, false),
            member("/a/same.RAF", Some((5, file("/o/same.jpg"))), false),
            member("/a/edited.RAF", Some((5, file("/o/edited.jpg"))), false),
            member("/a/gone.RAF", Some((5, file("/o/gone.jpg"))), true),
        ];
        let p = plan(
            &ms,
            &prints(&[("/a/new.RAF", 1), ("/a/same.RAF", 5), ("/a/edited.RAF", 6)]),
        );
        assert_eq!(
            p.export,
            vec![("/a/edited.RAF".into(), 0), ("/a/new.RAF".into(), 0)]
        );
        assert_eq!(p.clear.len(), 1);
        assert_eq!(p.replacing, HashSet::from([PathBuf::from("/o/edited.jpg")]));
        assert_eq!(p.prints.len(), 2);
    }

    #[test]
    fn modified_members_come_first() {
        let ms = [
            member("/a/a.RAF", None, false),
            member("/z/z.RAF", Some((1, file("/o/z.jpg"))), false),
        ];
        let p = plan(&ms, &prints(&[("/a/a.RAF", 1), ("/z/z.RAF", 2)]));
        assert_eq!(p.export[0].0, PathBuf::from("/z/z.RAF"));
    }

    /// Review focus 2: another member's file of the same name is never overwritten.
    #[test]
    fn two_stems_alike_keep_their_own_files() {
        let ms = [
            member("/a/a.RAF", Some((1, file("/o/a.jpg"))), false),
            member("/b/a.RAF", None, false),
        ];
        let p = plan(&ms, &prints(&[("/a/a.RAF", 1), ("/b/a.RAF", 1)]));
        assert_eq!(
            collision_answer(Path::new("/o/a.jpg"), &p.replacing),
            Collision::KeepBoth,
            "the new one would overwrite the published one's file"
        );
        let p = plan(&ms, &prints(&[("/a/a.RAF", 2), ("/b/a.RAF", 1)]));
        assert_eq!(
            collision_answer(Path::new("/o/a.jpg"), &p.replacing),
            Collision::Overwrite,
            "a re-publish did not take back its own file"
        );
    }

    fn report(items: Vec<WrittenItem>) -> ExportReport {
        ExportReport {
            written: items,
            ..Default::default()
        }
    }

    /// Review focus 3: only what the report says was delivered is recorded.
    #[test]
    fn only_written_photos_are_recorded() {
        let d = test_dir::TempDir::new("publish-run-record");
        let out = d.join("a.jpg");
        std::fs::write(&out, b"jpeg").unwrap();
        let (a, b) = (d.join("a.RAF"), d.join("b.RAF"));
        let ms = [
            member(a.to_str().unwrap(), None, false),
            member(b.to_str().unwrap(), None, false),
        ];
        let p = plan(
            &ms,
            &[((a.clone(), 0), 7u64), ((b.clone(), 0), 8)]
                .into_iter()
                .collect(),
        );
        let rows = delivered(
            &p,
            &report(vec![WrittenItem {
                id: PhotoId::original(a.clone()),
                file: Some(out.clone()),
                asset: None,
            }]),
            9,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, (a, 0));
        assert_eq!(rows[0].1.fingerprint, 7);
        assert!(matches!(&rows[0].1.place, Place::File { path, size: 4, .. } if *path == out));
    }

    /// Review focus 4: an upload that did not confirm is not in the report's `written` (the worker's rule,
    /// `an_unconfirmed_upload_is_not_in_written`), so nothing is recorded; an item with neither file nor
    /// asset is not either.
    #[test]
    fn an_unconfirmed_upload_is_not_recorded() {
        let a = PathBuf::from("/a/a.RAF");
        let p = plan(
            &[member("/a/a.RAF", None, false)],
            &prints(&[("/a/a.RAF", 1)]),
        );
        assert!(delivered(&p, &report(vec![]), 1).is_empty());
        let none = WrittenItem {
            id: PhotoId::original(a),
            file: None,
            asset: None,
        };
        assert!(delivered(&p, &report(vec![none]), 1).is_empty());
    }

    #[test]
    fn a_changed_file_is_left_and_counted() {
        let d = test_dir::TempDir::new("publish-run-clear");
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            d.path(),
            &[
                catalog::CatalogEntry {
                    path: d.join("a.RAF"),
                    file_name: "a.RAF".into(),
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    edited: None,
                },
                catalog::CatalogEntry {
                    path: d.join("b.RAF"),
                    file_name: "b.RAF".into(),
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    edited: None,
                },
            ],
        )
        .unwrap();
        let s = cat
            .create_publish_service(
                "S",
                &catalog::db::PublishTarget::Folder {
                    path: d.path().into(),
                },
                "{}",
            )
            .unwrap();
        for (photo, out) in [("a.RAF", "a.jpg"), ("b.RAF", "b.jpg")] {
            let f = d.join(out);
            std::fs::write(&f, b"jpeg").unwrap();
            cat.add_to_service(s, &d.join(photo), 0).unwrap();
            let place = publish_folder::place_of(&f).unwrap();
            cat.record_published(
                s,
                &d.join(photo),
                0,
                &Published {
                    fingerprint: 1,
                    place,
                    at: 1,
                },
            )
            .unwrap();
            cat.remove_from_service(s, &d.join(photo), 0).unwrap();
        }
        std::fs::write(d.join("b.jpg"), b"retouched").unwrap();
        let members = cat.publish_members(s).unwrap();
        assert_eq!(clear_folder(&cat, s, &members), (1, 1));
        assert!(!d.join("a.jpg").exists());
        assert!(d.join("b.jpg").exists());
        let left = cat.publish_members(s).unwrap();
        assert_eq!(left.len(), 1, "the left one stays Removed");
        assert!(left[0].removed && left[0].path == d.join("b.RAF"));
    }

    /// Records each call; answers every id `success`, except those in `refuse`.
    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
        refuse: HashSet<String>,
        unauthorized_create: bool,
    }

    impl Recorder {
        fn answer(&self, ids: &[String]) -> Vec<immich::BulkId> {
            ids.iter()
                .map(|id| immich::BulkId {
                    id: id.clone(),
                    success: !self.refuse.contains(id),
                    error: None,
                })
                .collect()
        }
    }

    impl Albums for Recorder {
        fn create(&self, name: &str) -> Result<String, immich::Error> {
            self.calls.borrow_mut().push(format!("create {name}"));
            if self.unauthorized_create {
                return Err(immich::Error::Unauthorized);
            }
            Ok("al1".into())
        }
        fn add(&self, album: &str, ids: &[String]) -> Result<Vec<immich::BulkId>, immich::Error> {
            self.calls
                .borrow_mut()
                .push(format!("add {album} {}", ids.join(",")));
            Ok(self.answer(ids))
        }
        fn remove(
            &self,
            album: &str,
            ids: &[String],
        ) -> Result<Vec<immich::BulkId>, immich::Error> {
            self.calls
                .borrow_mut()
                .push(format!("remove {album} {}", ids.join(",")));
            Ok(self.answer(ids))
        }
    }

    #[test]
    fn sync_album_creates_then_adds_then_removes() {
        let r = Recorder::default();
        let replaced = HashMap::from([("old".to_string(), "new".to_string())]);
        let s = sync_album(
            &r,
            None,
            "Web",
            &["new".into(), "fresh".into()],
            &replaced,
            &["gone".into()],
        )
        .unwrap();
        assert_eq!(
            *r.calls.borrow(),
            ["create Web", "add al1 new,fresh", "remove al1 gone,old"]
        );
        assert_eq!(s.album, "al1");
        assert_eq!(s.added, HashSet::from(["new".into(), "fresh".into()]));
        assert_eq!(s.removed, HashSet::from(["gone".into(), "old".into()]));
    }

    /// An old asset is taken out only once its replacement is in.
    #[test]
    fn a_replacement_that_did_not_go_in_keeps_the_old_one() {
        let r = Recorder {
            refuse: HashSet::from(["new".to_string()]),
            ..Default::default()
        };
        let replaced = HashMap::from([("old".to_string(), "new".to_string())]);
        let s = sync_album(&r, Some("al9"), "Web", &["new".into()], &replaced, &[]).unwrap();
        assert!(s.added.is_empty());
        assert_eq!(
            *r.calls.borrow(),
            ["add al9 new"],
            "the old one was taken out"
        );
    }

    #[test]
    fn a_refusal_names_the_permission() {
        let r = Recorder {
            unauthorized_create: true,
            ..Default::default()
        };
        let e = sync_album(&r, None, "Web", &[], &HashMap::new(), &[]).unwrap_err();
        assert_eq!(
            e,
            "Immich refused: the Write key needs the album.create permission"
        );
    }

    #[test]
    fn the_summary_reads_as_one_line() {
        let o = Outcome {
            published: 5,
            failed: 1,
            cleared: 1,
            left: 1,
            ..Default::default()
        };
        assert_eq!(
            o.summary(),
            "Published 5, 1 failed, removed 1, left 1 file changed since it was published"
        );
        assert_eq!(Outcome::default().summary(), "Published 0");
    }
}
