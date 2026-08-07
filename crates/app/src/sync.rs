//! Applying one photo's edits to many others.
//!
//! **No decoding happens here.** Every target's edits live in a JSON sidecar, so a sync is
//! `load → merge_from → save`. Syncing 200 photos is 200 tiny file operations rather than
//! 200 RAW decodes (~11 s each) — which is what makes a progress bar, a worker thread and
//! cancellation all unnecessary.

use crate::sidecar;
use edit_model::{EditGroups, Edits};
use std::path::PathBuf;

/// What a sync changed, and what it could not.
pub struct SyncReport {
    /// Each target's state *before* the sync, for undo.
    ///
    /// ⚠ `None` means the photo had **no sidecar at all**, so undoing must delete the file
    /// the sync created rather than writing a neutral one and leaving litter behind.
    pub prior: Vec<(PathBuf, Option<Edits>)>,
    /// Targets whose sidecar could not be written.
    pub failed: Vec<PathBuf>,
}

/// Apply `source`'s selected groups to every target's sidecar.
///
/// A failure on one photo does not abort the batch: a locked or unreachable file should not
/// cost the other targets their sync.
pub fn apply(targets: &[PathBuf], source: &Edits, groups: EditGroups) -> SyncReport {
    let mut prior = Vec::with_capacity(targets.len());
    let mut failed = Vec::new();
    for target in targets {
        let before = sidecar::load(target);
        let mut edits = before.clone().unwrap_or_default();
        edits.merge_from(source, groups);
        match sidecar::save(target, &edits) {
            Ok(()) => prior.push((target.clone(), before)),
            Err(_) => failed.push(target.clone()),
        }
    }
    SyncReport { prior, failed }
}

/// Reverse a sync, restoring each target's prior state. Returns the paths that failed.
pub fn revert(prior: &[(PathBuf, Option<Edits>)]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for (target, before) in prior {
        let ok = match before {
            Some(edits) => sidecar::save(target, edits).is_ok(),
            None => sidecar::remove(target).is_ok(),
        };
        if !ok {
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
        let report = apply(std::slice::from_ref(&a), &source(), EditGroups::only(1)); // Tone
        assert!(report.failed.is_empty());

        let got = sidecar::load(&a).expect("a sidecar should exist now");
        assert_eq!(got.exposure, 1.0, "tone should transfer");
        assert_eq!(
            got.vignette,
            edit_model::Vignette::default(),
            "effects were not selected"
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

        let report = apply(
            &[fresh.clone(), existing.clone()],
            &source(),
            EditGroups::ALL,
        );
        assert_eq!(report.prior.len(), 2);
        assert_eq!(report.prior[0], (fresh, None));
        assert_eq!(report.prior[1], (existing, Some(before)));
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

        let report = apply(
            &[fresh.clone(), existing.clone()],
            &source(),
            EditGroups::ALL,
        );
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

        let report = apply(&[bad.clone(), good.clone()], &source(), EditGroups::ALL);
        assert_eq!(report.failed, vec![bad]);
        assert_eq!(report.prior.len(), 1, "the good target still synced");
        assert!(sidecar::load(&good).is_some());
    }

    #[test]
    fn syncing_an_empty_target_list_does_nothing() {
        let report = apply(&[], &source(), EditGroups::ALL);
        assert!(report.prior.is_empty() && report.failed.is_empty());
        assert!(revert(&report.prior).is_empty());
    }
}
