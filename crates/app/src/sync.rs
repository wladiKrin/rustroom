//! Applying one photo's edits to many others.
//!
//! **No decoding happens here.** Every target's edits live in a JSON sidecar, so a sync is
//! `load → merge_from → save`. Syncing 200 photos is 200 tiny file operations rather than
//! 200 RAW decodes (~11 s each) — which is what makes a progress bar, a worker thread and
//! cancellation all unnecessary.

use crate::photo_id::PhotoId;
use crate::sidecar::{self, Sidecar};
use edit_model::{EditGroups, Edits};
use std::path::PathBuf;

/// What a sync changed, and what it could not.
pub struct SyncReport {
    /// Each target's state *before* the sync, for undo.
    ///
    /// ⚠⚠ The whole **file's** `Sidecar`, not one copy's `Edits`. Two copies of one file live in
    /// one sidecar, so a sync touching both performs two read-modify-write cycles on it — and a
    /// per-copy snapshot would restore only whichever was written last. An empty `Sidecar` means
    /// the photo had no sidecar at all, so undoing deletes the file the sync created rather than
    /// leaving a neutral one behind as litter.
    pub prior: Vec<(PathBuf, Sidecar)>,
    /// Targets whose sidecar could not be written.
    pub failed: Vec<PhotoId>,
}

/// Apply `source`'s selected groups to every target's sidecar.
///
/// A failure on one photo does not abort the batch: a locked or unreachable file should not
/// cost the other targets their sync.
pub fn apply(targets: &[PhotoId], source: &Edits, groups: EditGroups) -> SyncReport {
    let mut prior = Vec::with_capacity(targets.len());
    let mut failed = Vec::new();
    for target in targets {
        let before = sidecar::load_all(&target.path);
        let mut edits = before
            .get(target.copy)
            .map(|c| c.edits.clone())
            .unwrap_or_default();
        edits.merge_from(source, groups);
        let mut after = before.clone();
        after.set_edits(target.copy, edits);
        match sidecar::persist_all(&target.path, &after) {
            // ⚠ Only the *first* snapshot of a file is kept. A second target in the same file
            // would otherwise record a "prior" that already contains the first target's change,
            // and undoing would restore that instead of the original state.
            Ok(()) => {
                if !prior.iter().any(|(p, _)| *p == target.path) {
                    prior.push((target.path.clone(), before));
                }
            }
            Err(_) => failed.push(target.clone()),
        }
    }
    SyncReport { prior, failed }
}

/// Reverse a sync, restoring each target's prior state. Returns the paths that failed.
pub fn revert(prior: &[(PathBuf, Sidecar)]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for (target, before) in prior {
        // `persist_all` removes the file when the envelope is neutral, which covers the
        // "there was no sidecar" case without a separate branch.
        if sidecar::persist_all(target, before).is_err() {
            failed.push(target.clone());
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    ///
    /// The photo files themselves are never created: `sidecar::path` only appends `.json`,
    /// so a sync touches nothing but sidecars and needs no image fixtures at all.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_sync_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An original's id — the shape every pre-existing sync test means.
    fn orig(p: &std::path::Path) -> PhotoId {
        PhotoId::original(p.to_path_buf())
    }

    fn source() -> Edits {
        Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -50.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    #[test]
    fn a_sync_applies_only_the_selected_groups() {
        let d = TempDir::new("groups");
        let a = d.photo("a.RAF");
        let report = apply(&[orig(&a)], &source(), EditGroups::only(1)); // Tone
        assert!(report.failed.is_empty());

        let got = sidecar::load(&a).expect("a sidecar should exist now");
        assert_eq!(got.exposure, 1.0, "tone should transfer");
        assert_eq!(
            got.vignette,
            edit_model::Vignette::default(),
            "the vignette group was not selected"
        );
    }

    #[test]
    fn prior_records_none_for_a_photo_that_had_no_sidecar() {
        // ⚠ The load-bearing distinction: undoing must *delete* a sidecar the sync created,
        // not write a neutral one and leave litter behind.
        let d = TempDir::new("prior_none");
        let fresh = d.photo("fresh.RAF");
        let existing = d.photo("existing.RAF");
        let before = Edits {
            contrast: 25.0,
            ..Edits::default()
        };
        sidecar::save(&existing, &before).unwrap();

        let report = apply(&[orig(&fresh), orig(&existing)], &source(), EditGroups::ALL);
        assert_eq!(report.prior.len(), 2);
        // ⚠ An **empty** `Sidecar` is what "there was no sidecar" now looks like; `persist_all`
        // deletes the file for it, so revert still removes what the sync created.
        assert_eq!(report.prior[0], (fresh, Sidecar::default()));
        assert_eq!(
            report.prior[1].1.get(0).unwrap().edits,
            before,
            "the existing sidecar's prior edits"
        );
    }

    #[test]
    fn revert_restores_edited_sidecars_and_deletes_created_ones() {
        let d = TempDir::new("revert");
        let fresh = d.photo("fresh.RAF");
        let existing = d.photo("existing.RAF");
        let before = Edits {
            contrast: 25.0,
            ..Edits::default()
        };
        sidecar::save(&existing, &before).unwrap();

        let report = apply(&[orig(&fresh), orig(&existing)], &source(), EditGroups::ALL);
        assert!(sidecar::path(&fresh).exists());

        assert!(revert(&report.prior).is_empty());
        assert!(
            !sidecar::path(&fresh).exists(),
            "a sidecar the sync created must be removed, not neutralised"
        );
        assert_eq!(sidecar::load(&existing), Some(before));
    }

    #[test]
    fn a_failed_target_is_reported_without_aborting_the_batch() {
        // A locked or unreachable file should not cost the other photos their sync.
        let d = TempDir::new("failure");
        let good = d.photo("good.RAF");
        let bad = d.0.join("no_such_dir").join("bad.RAF");

        let report = apply(&[orig(&bad), orig(&good)], &source(), EditGroups::ALL);
        assert_eq!(report.failed, vec![orig(&bad)]);
        assert_eq!(report.prior.len(), 1, "the good target still synced");
        assert!(sidecar::load(&good).is_some());
    }

    #[test]
    fn syncing_an_empty_target_list_does_nothing() {
        let report = apply(&[], &source(), EditGroups::ALL);
        assert!(report.prior.is_empty() && report.failed.is_empty());
        assert!(revert(&report.prior).is_empty());
    }

    #[test]
    fn syncing_two_copies_of_one_file_restores_both_on_undo() {
        // ⚠ The case a per-copy `prior` gets wrong: both writes land in **one** file, so a
        // snapshot of one copy's `Edits` loses the other's original state and undo restores only
        // whichever was written last.
        use crate::sidecar::{CopyEntry, Sidecar};
        let d = TempDir::new("two_copies_undo");
        let photo = d.photo("a.RAF");
        let before = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    name: None,
                    edits: Edits {
                        exposure: 1.0,
                        ..Edits::default()
                    },
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                },
                CopyEntry {
                    copy: 1,
                    name: None,
                    edits: Edits {
                        exposure: 2.0,
                        ..Edits::default()
                    },
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                },
            ],
        };
        sidecar::save_all(&photo, &before).unwrap();

        let targets = vec![
            PhotoId::new(photo.clone(), 0),
            PhotoId::new(photo.clone(), 1),
        ];
        let report = apply(&targets, &source(), EditGroups::ALL);
        assert!(report.failed.is_empty());
        assert_eq!(
            report.prior.len(),
            1,
            "one snapshot per *file*, not per copy"
        );

        let after = sidecar::load_all(&photo);
        assert_eq!(after.get(0).unwrap().edits.exposure, 1.0, "source exposure");
        assert_eq!(after.get(1).unwrap().edits.exposure, 1.0);

        revert(&report.prior);

        let back = sidecar::load_all(&photo);
        assert_eq!(
            back.get(0).unwrap().edits.exposure,
            1.0,
            "copy 0 not restored"
        );
        assert_eq!(
            back.get(1).unwrap().edits.exposure,
            2.0,
            "copy 1 not restored — the per-file snapshot is what makes this work"
        );
    }
}
